//! Demo builds (`--features demo`) scan made-up buckets instead of AWS, for screenshots
//! that show no real data. `s3://` shows every demo bucket; `s3://acme-backups/` one.

use std::sync::Arc;
use std::time::Duration;

use clouddirstat_core::demo::{self, DemoBucket};
use clouddirstat_core::{Date, Entry};
use clouddirstat_providers::Error;
use clouddirstat_providers::s3::{BucketEstimate, Estimate, S3Pricing, ScanStats};
use tokio::sync::{mpsc, oneshot};

use crate::scan::ScanRequest;

const DEFAULT_OBJECTS: u64 = 120_000;
const SEED: u64 = 2026;
const BATCH: usize = 1000;
/// Pause per batch, so the scan looks (and can be captured) in progress.
const BATCH_DELAY: Duration = Duration::from_millis(40);
/// The day demo "CloudWatch" figures are from: 2026-08-31.
const DEMO_METRICS_DAY: u64 = 20_696;
/// Metrics a real estimate requests per bucket: 25 storage types and the object count.
const METRICS_PER_BUCKET: u64 = 26;

/// Set CLOUDDIRSTAT_DEMO_OBJECTS for bigger or smaller demos.
fn demo_objects() -> u64 {
    std::env::var("CLOUDDIRSTAT_DEMO_OBJECTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_OBJECTS)
}

/// What CloudWatch would report for the demo buckets: every version counted, as S3
/// counts them.
pub async fn estimate(request: &ScanRequest) -> clouddirstat_providers::Result<Estimate> {
    let objects = demo_objects();
    let location = &request.location;
    let buckets: Vec<(&DemoBucket, u64)> = if location.is_all_buckets() {
        demo::BUCKETS
            .iter()
            .map(|bucket| (bucket, demo::share(objects, bucket)))
            .collect()
    } else {
        vec![(find_bucket(&location.bucket)?, objects)]
    };

    let as_of = Date::from_unix_seconds(DEMO_METRICS_DAY * 86_400);
    let mut estimate = Estimate {
        metrics_requested: buckets.len() as u64 * METRICS_PER_BUCKET,
        ..Estimate::default()
    };
    for (bucket, share) in buckets {
        let mut classes: Vec<(String, u64)> = Vec::new();
        let mut count = 0;
        for entry in bucket.entries(share, true, SEED) {
            count += 1;
            match classes
                .iter_mut()
                .find(|(name, _)| *name == entry.storage_class)
            {
                Some((_, bytes)) => *bytes += entry.size,
                None => classes.push((entry.storage_class, entry.size)),
            }
        }
        let classes: Vec<(&str, u64)> = classes
            .iter()
            .map(|(class, bytes)| (class.as_str(), *bytes))
            .collect();
        estimate.buckets.push(BucketEstimate::from_classes(
            bucket.name.to_owned(),
            bucket.region.to_owned(),
            &classes,
            count,
            as_of,
        ));
    }
    estimate
        .buckets
        .sort_by_key(|bucket| std::cmp::Reverse(bucket.bytes()));
    Ok(estimate)
}

pub async fn run(
    request: &ScanRequest,
    pricing: oneshot::Sender<Arc<S3Pricing>>,
    expected_objects: oneshot::Sender<u64>,
    sender: mpsc::Sender<Vec<Entry>>,
) -> clouddirstat_providers::Result<ScanStats> {
    let objects = demo_objects();
    let location = &request.location;
    if location.prefix.is_empty() {
        let _ = expected_objects.send(estimate(request).await?.objects());
    }
    let versions = request.include_versions;

    let entries: Box<dyn Iterator<Item = Entry> + Send> = if location.is_all_buckets() {
        let regions = demo::BUCKETS
            .iter()
            .map(|bucket| (bucket.name, bucket.region));
        let _ = pricing.send(Arc::new(S3Pricing::for_buckets(regions)));
        Box::new(demo::all_buckets(objects, versions, SEED))
    } else {
        let bucket = find_bucket(&location.bucket)?;
        let _ = pricing.send(Arc::new(S3Pricing::for_region(bucket.region)));
        let prefix = location.prefix.clone();
        Box::new(
            bucket
                .entries(objects, versions, SEED)
                .filter(move |entry| entry.key.starts_with(&prefix)),
        )
    };

    let mut list_requests = 0;
    let mut batch = Vec::with_capacity(BATCH);
    for entry in entries {
        batch.push(entry);
        if batch.len() == BATCH {
            list_requests += 1;
            send(&sender, std::mem::take(&mut batch)).await?;
        }
    }
    if !batch.is_empty() {
        list_requests += 1;
        send(&sender, batch).await?;
    }
    Ok(ScanStats {
        list_requests,
        ..ScanStats::default()
    })
}

fn find_bucket(name: &str) -> clouddirstat_providers::Result<&'static DemoBucket> {
    demo::bucket(name).ok_or_else(|| {
        let names: Vec<_> = demo::BUCKETS.iter().map(|bucket| bucket.name).collect();
        Error::InvalidLocation(format!(
            "{name} (demo build: use s3:// or one of {})",
            names.join(", ")
        ))
    })
}

async fn send(
    sender: &mpsc::Sender<Vec<Entry>>,
    batch: Vec<Entry>,
) -> clouddirstat_providers::Result<()> {
    tokio::time::sleep(BATCH_DELAY).await;
    sender.send(batch).await.map_err(|_| Error::Cancelled)
}
