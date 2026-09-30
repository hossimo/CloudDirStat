//! Bucket totals from the daily S3 storage metrics in CloudWatch: size by storage
//! class, object count, and cost, without listing anything.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use aws_config::{Region, SdkConfig};
use aws_sdk_cloudwatch::Client;
use aws_sdk_cloudwatch::primitives::DateTime;
use aws_sdk_cloudwatch::types::{Dimension, Metric, MetricDataQuery, MetricStat, ScanBy};
use clouddirstat_core::{Cost, Date};
use tokio::task::JoinSet;

use super::pricing::{INT_ARCHIVE, INT_ARCHIVE_INSTANT, INT_DEEP_ARCHIVE, INT_INFREQUENT};
use super::{LIST_PRICE_PER_1000_USD, S3Pricing, request_error};
use crate::{Result, SkippedBucket};

/// CloudWatch bills GetMetricData per metric requested.
const METRIC_PRICE_PER_1000_USD: f64 = 0.01;
/// S3 posts storage metrics once a day, a day or two late.
const LOOKBACK: Duration = Duration::from_secs(4 * 86_400);
const DAY_SECONDS: i32 = 86_400;
const MAX_QUERIES_PER_REQUEST: usize = 500;
/// Objects returned per LIST request.
const OBJECTS_PER_LIST: u64 = 1000;

/// CloudWatch `StorageType`s: the storage class they are shown under, and the class
/// whose price applies. Per-object overheads of archive tiers are billed at Standard,
/// and each Intelligent-Tiering tier at its own rate.
const STORAGE_TYPES: &[(&str, &str, &str)] = &[
    ("StandardStorage", "STANDARD", "STANDARD"),
    ("IntelligentTieringFAStorage", INT, INT),
    ("IntelligentTieringIAStorage", INT, INT_INFREQUENT),
    ("IntelligentTieringAIAStorage", INT, INT_ARCHIVE_INSTANT),
    ("IntelligentTieringAAStorage", INT, INT_ARCHIVE),
    ("IntelligentTieringDAAStorage", INT, INT_DEEP_ARCHIVE),
    ("IntAAObjectOverhead", INT, INT_ARCHIVE),
    ("IntAAS3ObjectOverhead", INT, "STANDARD"),
    ("IntDAAObjectOverhead", INT, INT_DEEP_ARCHIVE),
    ("IntDAAS3ObjectOverhead", INT, "STANDARD"),
    ("StandardIAStorage", "STANDARD_IA", "STANDARD_IA"),
    ("StandardIASizeOverhead", "STANDARD_IA", "STANDARD_IA"),
    ("OneZoneIAStorage", "ONEZONE_IA", "ONEZONE_IA"),
    ("OneZoneIASizeOverhead", "ONEZONE_IA", "ONEZONE_IA"),
    (
        "ReducedRedundancyStorage",
        "REDUCED_REDUNDANCY",
        "REDUCED_REDUNDANCY",
    ),
    ("GlacierInstantRetrievalStorage", "GLACIER_IR", "GLACIER_IR"),
    ("GlacierIRSizeOverhead", "GLACIER_IR", "GLACIER_IR"),
    ("GlacierStorage", "GLACIER", "GLACIER"),
    ("GlacierObjectOverhead", "GLACIER", "GLACIER"),
    ("GlacierS3ObjectOverhead", "GLACIER", "STANDARD"),
    ("GlacierStagingStorage", "GLACIER", "STANDARD"),
    ("DeepArchiveStorage", "DEEP_ARCHIVE", "DEEP_ARCHIVE"),
    ("DeepArchiveObjectOverhead", "DEEP_ARCHIVE", "DEEP_ARCHIVE"),
    ("DeepArchiveS3ObjectOverhead", "DEEP_ARCHIVE", "STANDARD"),
    ("DeepArchiveStagingStorage", "DEEP_ARCHIVE", "STANDARD"),
];
const INT: &str = "INTELLIGENT_TIERING";

/// How much to ask CloudWatch for. It bills per metric, so counting objects alone is
/// 26 times cheaper than a full breakdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Detail {
    /// Bytes per storage type, and the object count.
    Full,
    /// Only the object count.
    ObjectCount,
}

impl Detail {
    fn queries_per_bucket(self) -> usize {
        match self {
            Self::Full => STORAGE_TYPES.len() + 1,
            Self::ObjectCount => 1,
        }
    }
}

/// One bucket's totals as CloudWatch last reported them.
#[derive(Clone, Debug, Default)]
pub struct BucketEstimate {
    pub bucket: String,
    pub region: String,
    /// Bytes per storage class, largest first, including billed per-object overheads.
    pub classes: Vec<(String, u64)>,
    /// Every object version, delete marker, and uploaded part. `None` when CloudWatch
    /// has no data yet (buckets appear about a day after they get their first object).
    pub objects: Option<u64>,
    pub monthly_cost: Cost,
    /// The day the figures are from.
    pub as_of: Option<Date>,
}

impl BucketEstimate {
    /// Builds a bucket's totals from the latest value of each storage type (indexes
    /// into [`STORAGE_TYPES`]) and of the object count.
    pub fn new(
        bucket: String,
        region: String,
        sizes: &[(usize, u64)],
        objects: Option<u64>,
        as_of: Option<Date>,
    ) -> Self {
        let pricing = S3Pricing::for_region(&region);
        let mut classes: Vec<(String, u64)> = Vec::new();
        let mut monthly_cost = Cost::ZERO;
        for &(index, bytes) in sizes {
            let (_, class, price_class) = STORAGE_TYPES[index];
            monthly_cost += pricing.bytes_cost(price_class, bytes);
            match classes.iter_mut().find(|(name, _)| name == class) {
                Some((_, total)) => *total += bytes,
                None => classes.push((class.to_owned(), bytes)),
            }
        }
        classes.retain(|&(_, bytes)| bytes > 0);
        classes.sort_by_key(|&(_, bytes)| Reverse(bytes));
        Self {
            bucket,
            region,
            classes,
            objects,
            monthly_cost,
            as_of,
        }
    }

    /// Totals for made-up data (demo builds): bytes per storage class and objects.
    pub fn from_classes(
        bucket: String,
        region: String,
        classes: &[(&str, u64)],
        objects: u64,
        as_of: Option<Date>,
    ) -> Self {
        let sizes: Vec<(usize, u64)> = classes
            .iter()
            .filter_map(|&(class, bytes)| {
                let index = STORAGE_TYPES
                    .iter()
                    .position(|&(_, shown, priced)| shown == class && priced == class)?;
                Some((index, bytes))
            })
            .collect();
        Self::new(bucket, region, &sizes, Some(objects), as_of)
    }

    pub fn bytes(&self) -> u64 {
        self.classes.iter().map(|&(_, bytes)| bytes).sum()
    }

    /// LIST requests a full scan needs: one per 1,000 objects, plus one for incomplete
    /// uploads. Without versions, noncurrent versions are not listed, so fewer.
    pub fn list_requests(&self) -> u64 {
        self.objects.unwrap_or(0).div_ceil(OBJECTS_PER_LIST).max(1) + 1
    }
}

/// Totals for every bucket of a location.
#[derive(Clone, Debug, Default)]
pub struct Estimate {
    /// Largest first.
    pub buckets: Vec<BucketEstimate>,
    pub skipped: Vec<SkippedBucket>,
    /// Metrics requested from CloudWatch, which bills $0.01 per 1,000.
    pub metrics_requested: u64,
}

impl Estimate {
    pub fn bytes(&self) -> u64 {
        self.buckets.iter().map(BucketEstimate::bytes).sum()
    }

    pub fn objects(&self) -> u64 {
        self.buckets
            .iter()
            .filter_map(|bucket| bucket.objects)
            .sum()
    }

    pub fn monthly_cost(&self) -> Cost {
        let mut total = Cost::ZERO;
        for bucket in &self.buckets {
            total += bucket.monthly_cost;
        }
        total
    }

    pub fn list_requests(&self) -> u64 {
        self.buckets.iter().map(BucketEstimate::list_requests).sum()
    }

    pub fn scan_cost_usd(&self) -> f64 {
        list_cost_usd(self.list_requests())
    }

    /// What getting this estimate cost.
    pub fn request_cost_usd(&self) -> f64 {
        self.metrics_requested as f64 / 1000.0 * METRIC_PRICE_PER_1000_USD
    }

    /// The oldest day any bucket's figures are from.
    pub fn as_of(&self) -> Option<Date> {
        self.buckets.iter().filter_map(|bucket| bucket.as_of).min()
    }

    /// Storage classes over all buckets, largest first.
    pub fn classes(&self) -> Vec<(String, u64)> {
        let mut totals: Vec<(String, u64)> = Vec::new();
        for (class, bytes) in self.buckets.iter().flat_map(|bucket| &bucket.classes) {
            match totals.iter_mut().find(|(name, _)| name == class) {
                Some((_, total)) => *total += bytes,
                None => totals.push((class.clone(), *bytes)),
            }
        }
        totals.sort_by_key(|&(_, bytes)| Reverse(bytes));
        totals
    }
}

pub fn list_cost_usd(requests: u64) -> f64 {
    requests as f64 / 1000.0 * LIST_PRICE_PER_1000_USD
}

/// Fetches the latest totals of each `(bucket, region)`. A region whose request fails
/// skips its buckets; if every request fails, that error is returned.
pub(super) async fn fetch(
    config: &SdkConfig,
    targets: &[(String, String)],
    detail: Detail,
) -> Result<Estimate> {
    let mut by_region: HashMap<&str, Vec<&str>> = HashMap::new();
    for (bucket, region) in targets {
        by_region.entry(region).or_default().push(bucket);
    }

    let end = SystemTime::now();
    let start = end - LOOKBACK;
    let mut tasks = JoinSet::new();
    for (region, buckets) in by_region {
        let client = Client::from_conf(
            aws_sdk_cloudwatch::config::Builder::from(config)
                .region(Region::new(region.to_owned()))
                .build(),
        );
        for chunk in buckets.chunks(MAX_QUERIES_PER_REQUEST / detail.queries_per_bucket()) {
            let client = client.clone();
            let region = region.to_owned();
            let buckets: Vec<String> = chunk.iter().map(|&bucket| bucket.to_owned()).collect();
            tasks.spawn(async move {
                let result = fetch_chunk(&client, &region, &buckets, detail, start, end).await;
                (buckets, result)
            });
        }
    }

    let mut estimate = Estimate::default();
    let mut first_error = None;
    while let Some(finished) = tasks.join_next().await {
        let (buckets, result) = finished?;
        estimate.metrics_requested += (buckets.len() * detail.queries_per_bucket()) as u64;
        match result {
            Ok(found) => estimate.buckets.extend(found),
            Err(error) => {
                estimate
                    .skipped
                    .extend(buckets.into_iter().map(|bucket| SkippedBucket {
                        bucket,
                        reason: error.to_string(),
                    }));
                first_error.get_or_insert(error);
            }
        }
    }
    if estimate.buckets.is_empty()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    estimate
        .buckets
        .sort_by_key(|bucket| Reverse(bucket.bytes()));
    Ok(estimate)
}

async fn fetch_chunk(
    client: &Client,
    region: &str,
    buckets: &[String],
    detail: Detail,
    start: SystemTime,
    end: SystemTime,
) -> Result<Vec<BucketEstimate>> {
    let mut queries = Vec::with_capacity(buckets.len() * detail.queries_per_bucket());
    for (index, bucket) in buckets.iter().enumerate() {
        let storage_types = match detail {
            Detail::Full => STORAGE_TYPES,
            Detail::ObjectCount => &[],
        };
        for (type_index, &(storage_type, _, _)) in storage_types.iter().enumerate() {
            queries.push(query(
                format!("b{index}t{type_index}"),
                bucket,
                "BucketSizeBytes",
                storage_type,
            ));
        }
        queries.push(query(
            format!("b{index}n"),
            bucket,
            "NumberOfObjects",
            "AllStorageTypes",
        ));
    }

    let mut pages = client
        .get_metric_data()
        .set_metric_data_queries(Some(queries))
        .start_time(DateTime::from(start))
        .end_time(DateTime::from(end))
        .scan_by(ScanBy::TimestampDescending)
        .into_paginator()
        .send();
    // The newest value of each query, and when it was measured.
    let mut latest: HashMap<String, (f64, i64)> = HashMap::new();
    while let Some(page) = pages.next().await {
        let page = page
            .map_err(|error| request_error("GetMetricData", "cloudwatch:GetMetricData", error))?;
        for result in page.metric_data_results() {
            let (Some(id), Some(&value), Some(time)) = (
                result.id(),
                result.values().first(),
                result.timestamps().first(),
            ) else {
                continue;
            };
            latest.entry(id.to_owned()).or_insert((value, time.secs()));
        }
    }

    let found = buckets.iter().enumerate().map(|(index, bucket)| {
        let value = |id: String| latest.get(&id).copied();
        let sizes: Vec<(usize, u64)> = (0..STORAGE_TYPES.len())
            .filter_map(|type_index| {
                let (bytes, _) = value(format!("b{index}t{type_index}"))?;
                Some((type_index, bytes as u64))
            })
            .collect();
        let objects = value(format!("b{index}n"));
        let newest = (0..STORAGE_TYPES.len())
            .filter_map(|type_index| value(format!("b{index}t{type_index}")))
            .chain(objects)
            .map(|(_, time)| time)
            .max();
        let as_of = newest
            .and_then(|time| u64::try_from(time).ok())
            .and_then(Date::from_unix_seconds);
        BucketEstimate::new(
            bucket.clone(),
            region.to_owned(),
            &sizes,
            objects.map(|(count, _)| count as u64),
            as_of,
        )
    });
    Ok(found.collect())
}

fn query(id: String, bucket: &str, metric: &str, storage_type: &str) -> MetricDataQuery {
    let dimension = |name: &str, value: &str| Dimension::builder().name(name).value(value).build();
    let metric = Metric::builder()
        .namespace("AWS/S3")
        .metric_name(metric)
        .dimensions(dimension("BucketName", bucket))
        .dimensions(dimension("StorageType", storage_type))
        .build();
    MetricDataQuery::builder()
        .id(id)
        .metric_stat(
            MetricStat::builder()
                .metric(metric)
                .period(DAY_SECONDS)
                .stat("Average")
                .build(),
        )
        .return_data(true)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn index(storage_type: &str) -> usize {
        STORAGE_TYPES
            .iter()
            .position(|&(name, _, _)| name == storage_type)
            .unwrap()
    }

    #[test]
    fn groups_storage_types_into_classes_and_prices_overheads_at_standard() {
        let estimate = BucketEstimate::new(
            "b".to_owned(),
            "us-east-1".to_owned(),
            &[
                (index("StandardStorage"), 10 * GIB),
                (index("DeepArchiveStorage"), 100 * GIB),
                (index("DeepArchiveObjectOverhead"), GIB),
                (index("DeepArchiveS3ObjectOverhead"), GIB),
            ],
            Some(2500),
            None,
        );

        assert_eq!(
            estimate.classes,
            [
                ("DEEP_ARCHIVE".to_owned(), 102 * GIB),
                ("STANDARD".to_owned(), 10 * GIB)
            ]
        );
        let pricing = S3Pricing::for_region("us-east-1");
        let mut expected = pricing.bytes_cost("STANDARD", 11 * GIB);
        expected += pricing.bytes_cost("DEEP_ARCHIVE", 101 * GIB);
        assert_eq!(estimate.monthly_cost, expected);
        assert_eq!(estimate.list_requests(), 3 + 1);
    }

    #[test]
    fn prices_each_intelligent_tiering_tier_at_its_own_rate() {
        let estimate = BucketEstimate::new(
            "b".to_owned(),
            "us-east-1".to_owned(),
            &[
                (index("IntelligentTieringFAStorage"), GIB),
                (index("IntelligentTieringDAAStorage"), 100 * GIB),
            ],
            None,
            None,
        );

        assert_eq!(
            estimate.classes,
            [("INTELLIGENT_TIERING".to_owned(), 101 * GIB)]
        );
        let pricing = S3Pricing::for_region("us-east-1");
        let mut expected = pricing.bytes_cost(INT, GIB);
        expected += pricing.bytes_cost(INT_DEEP_ARCHIVE, 100 * GIB);
        assert_eq!(estimate.monthly_cost, expected);
        assert!(estimate.monthly_cost < pricing.bytes_cost(INT, 101 * GIB));
    }

    #[test]
    fn buckets_without_data_still_need_a_request() {
        let estimate = BucketEstimate::new("b".into(), "us-east-1".into(), &[], None, None);
        assert_eq!(estimate.bytes(), 0);
        assert_eq!(estimate.list_requests(), 2);
    }

    #[test]
    fn requests_fit_whole_buckets() {
        assert_eq!(
            MAX_QUERIES_PER_REQUEST / Detail::Full.queries_per_bucket(),
            19
        );
        assert_eq!(
            MAX_QUERIES_PER_REQUEST / Detail::ObjectCount.queries_per_bucket(),
            500
        );
    }
}
