//! What every provider's scan shares: splitting a bucket into prefixes that are listed in
//! parallel, scanning several buckets at once, and the counters, request slots,
//! warnings, and sink that listers report to.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clouddirstat_core::Entry;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::task::JoinSet;

use crate::{EntrySender, Error, Result, ScanStats, SkippedBucket};

/// How many folder levels deep a bucket may be split to keep every slot busy.
const MAX_SPLIT_DEPTH: usize = 3;
/// Buckets (or containers) in progress at the same time when scanning more than one.
/// They share the request slots, so a big one gets all of them once the small ones are
/// done.
const BUCKETS_AT_ONCE: usize = 16;

/// Lists one prefix of one bucket at a provider.
pub(crate) trait PrefixLister: Clone + Send + Sync + 'static {
    /// Lists everything under `prefix` and sends it to the tree. With `split`, lists one
    /// level only (`/` as delimiter) and returns the prefixes one level down.
    fn list(&self, prefix: String, split: bool)
    -> impl Future<Output = Result<Vec<String>>> + Send;
}

/// Lists everything under `prefix`: expands prefixes breadth-first until there are
/// enough to keep `concurrency` requests busy, then lists each one fully in parallel.
pub(crate) async fn list_prefix(
    lister: impl PrefixLister,
    prefix: String,
    concurrency: usize,
) -> Result<()> {
    let concurrency = concurrency.max(1);

    let mut prefixes = vec![prefix];
    for _ in 0..MAX_SPLIT_DEPTH {
        if prefixes.is_empty() || prefixes.len() >= concurrency {
            break;
        }
        // Each level's prefixes are listed in parallel, within the shared slots.
        let mut level = JoinSet::new();
        for prefix in prefixes {
            let lister = lister.clone();
            level.spawn(async move { lister.list(prefix, true).await });
        }
        let mut subprefixes = Vec::new();
        while let Some(found) = level.join_next().await {
            subprefixes.extend(found??);
        }
        prefixes = subprefixes;
    }

    let mut tasks = JoinSet::new();
    for prefix in prefixes {
        if tasks.len() >= concurrency
            && let Some(finished) = tasks.join_next().await
        {
            finished??;
        }
        let lister = lister.clone();
        tasks.spawn(async move { lister.list(prefix, false).await.map(drop) });
    }
    while let Some(finished) = tasks.join_next().await {
        finished??;
    }
    Ok(())
}

/// Runs each named bucket's scan, [`BUCKETS_AT_ONCE`] at a time. A bucket that fails is
/// added to `skipped` instead of ending the scan; cancellation still stops everything.
pub(crate) async fn each_bucket<F>(
    scans: impl IntoIterator<Item = (String, F)>,
    skipped: &mut Vec<SkippedBucket>,
) -> Result<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    let mut tasks = JoinSet::new();
    for (bucket, scan) in scans {
        if tasks.len() >= BUCKETS_AT_ONCE
            && let Some(finished) = tasks.join_next().await
        {
            record_bucket_result(finished?, skipped)?;
        }
        tasks.spawn(async move { (bucket, scan.await) });
    }
    while let Some(finished) = tasks.join_next().await {
        record_bucket_result(finished?, skipped)?;
    }
    Ok(())
}

fn record_bucket_result(
    (bucket, result): (String, Result<()>),
    skipped: &mut Vec<SkippedBucket>,
) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(error) => {
            skipped.push(SkippedBucket {
                bucket,
                reason: error.to_string(),
            });
            Ok(())
        }
    }
}

/// What every lister of one scan shares: request slots, request and cost counters,
/// warnings, and where entries go. Cloning it is cheap; the clones share everything.
#[derive(Clone)]
pub(crate) struct ScanContext {
    /// One slot per listing in progress, across every bucket of the scan.
    slots: Arc<Semaphore>,
    requests: Arc<AtomicU64>,
    cost_nanodollars: Arc<AtomicU64>,
    warnings: Arc<Mutex<Vec<String>>>,
    sink: EntrySender,
}

impl ScanContext {
    /// Starts with the requests made while connecting, each at `setup_price_per_1000`
    /// USD per 1,000, and the warnings found then.
    pub fn new(
        concurrency: usize,
        sink: EntrySender,
        setup_requests: u64,
        setup_price_per_1000: f64,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(concurrency.max(1))),
            requests: Arc::new(AtomicU64::new(setup_requests)),
            cost_nanodollars: Arc::new(AtomicU64::new(
                setup_requests * nanodollars(setup_price_per_1000),
            )),
            warnings: Arc::new(Mutex::new(warnings)),
            sink,
        }
    }

    /// Waits for a free request slot, which is held until the permit is dropped.
    pub async fn slot(&self) -> Result<SemaphorePermit<'_>> {
        self.slots.acquire().await.map_err(|_| Error::Cancelled)
    }

    /// Counts one LIST request at `price_per_1000` USD per 1,000.
    pub fn count_request(&self, price_per_1000: f64) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.cost_nanodollars
            .fetch_add(nanodollars(price_per_1000), Ordering::Relaxed);
    }

    pub fn warn(&self, warning: String) {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(warning);
        }
    }

    /// Hands entries to the tree; fails only when the scan was cancelled.
    pub async fn send(&self, entries: Vec<Entry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.sink.send(entries).await.map_err(|_| Error::Cancelled)
    }

    /// The totals of the finished scan. With no requests made, the price per 1,000 is
    /// `default_price_per_1000`; otherwise the average of the requests made.
    pub fn stats(&self, skipped: Vec<SkippedBucket>, default_price_per_1000: f64) -> ScanStats {
        let list_requests = self.requests.load(Ordering::Relaxed);
        let cost_usd = self.cost_nanodollars.load(Ordering::Relaxed) as f64 / 1e9;
        ScanStats {
            list_requests,
            list_price_per_1000_usd: if list_requests == 0 {
                default_price_per_1000
            } else {
                cost_usd * 1000.0 / list_requests as f64
            },
            skipped_buckets: skipped,
            warnings: self
                .warnings
                .lock()
                .map(|list| list.clone())
                .unwrap_or_default(),
        }
    }
}

/// The cost of one request in nano-dollars, given the price of 1,000.
fn nanodollars(price_per_1000: f64) -> u64 {
    (price_per_1000 * 1e6).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_requests_at_their_average() {
        let (sink, _receiver) = tokio::sync::mpsc::channel(1);
        let context = ScanContext::new(4, sink, 2, 0.005, vec!["setup".to_owned()]);
        context.count_request(0.005);
        context.count_request(0.05);
        let stats = context.stats(Vec::new(), 1.0);
        assert_eq!(stats.list_requests, 4);
        assert!((stats.list_price_per_1000_usd - 0.01625).abs() < 1e-9);
        assert_eq!(stats.warnings, ["setup"]);

        let (sink, _receiver) = tokio::sync::mpsc::channel(1);
        let idle = ScanContext::new(4, sink, 0, 0.0, Vec::new());
        assert_eq!(idle.stats(Vec::new(), 0.02).list_price_per_1000_usd, 0.02);
    }

    #[test]
    fn a_failed_bucket_is_skipped_but_cancelling_stops_the_scan() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let outcome = |results: Vec<Result<()>>| {
            let scans = results
                .into_iter()
                .enumerate()
                .map(|(index, result)| (format!("b{index}"), async move { result }));
            let mut skipped = Vec::new();
            let result = runtime.block_on(each_bucket(scans, &mut skipped));
            (result, skipped)
        };

        let (result, skipped) = outcome(vec![Ok(()), Err(Error::NoSuchBucket("b1".into()))]);
        assert!(result.is_ok());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].bucket, "b1");

        let (result, _) = outcome(vec![Err(Error::Cancelled)]);
        assert!(matches!(result, Err(Error::Cancelled)));
    }
}
