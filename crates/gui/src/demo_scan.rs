//! Demo builds (`--features demo`) scan made-up buckets instead of the cloud, for
//! screenshots that show no real data. `s3://` shows every demo bucket;
//! `s3://acme-backups/` one. `gs://` shows the same buckets as Google Cloud Storage, and
//! `az://demo/` as the containers of an Azure storage account.

use std::sync::Arc;
use std::time::Duration;

use clouddirstat_core::Pricing;
use clouddirstat_core::demo::{self, DemoBucket};
use clouddirstat_core::{Date, Entry, EntryKind};
use clouddirstat_providers::azure::{AccountPlacement, AzurePricing};
use clouddirstat_providers::gcs::{BucketPlacement, GcsPricing};
use clouddirstat_providers::s3::{BucketEstimate, Estimate, LIST_PRICE_PER_1000_USD, S3Pricing};
use clouddirstat_providers::{Error, Provider, ScanStats};
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
    let buckets: Vec<(&DemoBucket, u64)> = if location.is_all() {
        demo::BUCKETS
            .iter()
            .map(|bucket| (bucket, demo::share(objects, bucket)))
            .collect()
    } else {
        vec![(find_bucket(location.bucket().unwrap_or_default())?, objects)]
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
                None => classes.push((entry.storage_class.into_owned(), entry.size)),
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
    pricing: oneshot::Sender<Arc<dyn Pricing>>,
    expected_objects: oneshot::Sender<u64>,
    sender: mpsc::Sender<Vec<Entry>>,
) -> clouddirstat_providers::Result<ScanStats> {
    let objects = demo_objects();
    let location = &request.location;
    let provider = location.provider();
    // Progress comes from CloudWatch, which only S3 has.
    if location.prefix().is_empty() && provider == Provider::S3 {
        let _ = expected_objects.send(estimate(request).await?.objects());
    }
    let versions = request.include_versions;

    let entries: Box<dyn Iterator<Item = Entry> + Send> = if location.is_all() {
        let _ = pricing.send(demo_pricing(provider, None));
        Box::new(demo::all_buckets(objects, versions, SEED))
    } else {
        let bucket = find_bucket(location.bucket().unwrap_or_default())?;
        let _ = pricing.send(demo_pricing(provider, Some(bucket)));
        let prefix = location.prefix().to_owned();
        Box::new(
            bucket
                .entries(objects, versions, SEED)
                .filter(move |entry| entry.key.starts_with(&prefix)),
        )
    };
    let entries = entries.filter_map(move |entry| as_listed_by(provider, entry));

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
        list_price_per_1000_usd: match provider {
            Provider::S3 => LIST_PRICE_PER_1000_USD,
            Provider::Gcs => GcsPricing::list_price_per_1000("STANDARD"),
            Provider::Azure => AzurePricing::list_price_per_1000(&east_us()),
        },
        ..ScanStats::default()
    })
}

/// Prices for one demo bucket, or all of them. Google buckets are in us-central1 and
/// Azure containers in an eastus LRS account; S3 buckets have regions of their own.
fn demo_pricing(provider: Provider, bucket: Option<&DemoBucket>) -> Arc<dyn Pricing> {
    match (provider, bucket) {
        (Provider::S3, Some(bucket)) => Arc::new(S3Pricing::for_region(bucket.region)),
        (Provider::S3, None) => Arc::new(S3Pricing::for_buckets(
            demo::BUCKETS
                .iter()
                .map(|bucket| (bucket.name, bucket.region)),
        )),
        (Provider::Gcs, Some(_)) => Arc::new(GcsPricing::for_bucket(&iowa())),
        (Provider::Gcs, None) => {
            let placement = iowa();
            let names = demo::BUCKETS.iter().map(|bucket| (bucket.name, &placement));
            Arc::new(GcsPricing::for_buckets(names))
        }
        (Provider::Azure, _) => Arc::new(AzurePricing::for_account(&east_us())),
    }
}

fn iowa() -> BucketPlacement {
    BucketPlacement {
        location: "US-CENTRAL1".to_owned(),
        location_type: "region".to_owned(),
        data_locations: Vec::new(),
    }
}

fn east_us() -> AccountPlacement {
    AccountPlacement {
        region: "eastus".to_owned(),
        sku: "Standard_LRS".to_owned(),
    }
}

/// A demo S3 entry as `provider` would list it: the storage class (or Azure access
/// tier) of the same temperature, and without delete markers (which only S3 has) or
/// incomplete uploads (which are only found on S3 so far).
fn as_listed_by(provider: Provider, mut entry: Entry) -> Option<Entry> {
    if provider == Provider::S3 {
        return Some(entry);
    }
    if matches!(
        entry.kind,
        EntryKind::DeleteMarker | EntryKind::IncompleteUpload
    ) {
        return None;
    }
    let temperature = match entry.storage_class.as_ref() {
        "STANDARD_IA" | "ONEZONE_IA" => 1,
        "GLACIER_IR" => 2,
        "GLACIER" | "DEEP_ARCHIVE" => 3,
        _ => 0,
    };
    let classes = match provider {
        Provider::Gcs => ["STANDARD", "NEARLINE", "COLDLINE", "ARCHIVE"],
        _ => ["Hot", "Cool", "Cold", "Archive"],
    };
    entry.storage_class = classes[temperature].into();
    Some(entry)
}

fn find_bucket(name: &str) -> clouddirstat_providers::Result<&'static DemoBucket> {
    demo::bucket(name).ok_or_else(|| {
        Error::Unsupported(format!(
            "there is no {name} in the demo build, which only has made-up data.\n\nTry:\n\
             - s3://\n- gs://\n- az://demo/"
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
