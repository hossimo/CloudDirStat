//! Demo builds (`--features demo`) scan made-up buckets instead of AWS, for screenshots
//! that show no real data. `s3://` shows every demo bucket; `s3://acme-backups/` one.

use std::sync::Arc;
use std::time::Duration;

use clouddirstat_core::Entry;
use clouddirstat_core::demo::{self, DemoBucket};
use clouddirstat_providers::Error;
use clouddirstat_providers::s3::{S3Pricing, ScanStats};
use tokio::sync::{mpsc, oneshot};

use crate::scan::ScanRequest;

const DEFAULT_OBJECTS: u64 = 120_000;
const SEED: u64 = 2026;
const BATCH: usize = 1000;
/// Pause per batch, so the scan looks (and can be captured) in progress.
const BATCH_DELAY: Duration = Duration::from_millis(40);

pub async fn run(
    request: &ScanRequest,
    pricing: oneshot::Sender<Arc<S3Pricing>>,
    sender: mpsc::Sender<Vec<Entry>>,
) -> clouddirstat_providers::Result<ScanStats> {
    // Set CLOUDDIRSTAT_DEMO_OBJECTS for bigger or smaller demos.
    let objects = std::env::var("CLOUDDIRSTAT_DEMO_OBJECTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_OBJECTS);
    let location = &request.location;
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
        skipped_buckets: Vec::new(),
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
