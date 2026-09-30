mod credentials;
mod location;
mod prices;
mod pricing;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::ProvideCredentials;
use aws_sdk_s3::error::DisplayErrorContext;
use aws_sdk_s3::primitives::DateTime;
use aws_sdk_s3::types::CommonPrefix;
use clouddirstat_core::{Entry, EntryKind};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;

pub use credentials::{AccessKey, CredentialSource};
pub use location::S3Location;
pub use pricing::S3Pricing;

use crate::{Error, Result};

const LIST_PRICE_PER_1000_USD: f64 = 0.005;
const DEFAULT_STORAGE_CLASS: &str = "STANDARD";
const MAX_SPLIT_DEPTH: usize = 3;
/// Buckets in progress at the same time in an all-buckets scan. They share the
/// request slots, so a big bucket gets all of them once the small ones are done.
const BUCKETS_AT_ONCE: usize = 16;

pub type EntrySender = Sender<Vec<Entry>>;

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub include_versions: bool,
    pub concurrency: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ScanStats {
    pub list_requests: u64,
    /// Buckets left out of an all-buckets scan, e.g. for lack of `s3:ListBucket`.
    pub skipped_buckets: Vec<SkippedBucket>,
    /// Optional checks that could not run, e.g. incomplete uploads without
    /// `s3:ListBucketMultipartUploads`. The scan itself still completed.
    pub warnings: Vec<String>,
}

impl ScanStats {
    pub fn estimated_cost_usd(&self) -> f64 {
        self.list_requests as f64 / 1000.0 * LIST_PRICE_PER_1000_USD
    }
}

#[derive(Clone, Debug)]
pub struct SkippedBucket {
    pub bucket: String,
    pub reason: String,
}

/// Lists one bucket, or every bucket the credentials can see.
pub struct S3Scanner {
    location: S3Location,
    targets: Vec<Target>,
    skipped: Vec<SkippedBucket>,
    setup_requests: u64,
}

struct Target {
    bucket: String,
    region: String,
    client: Client,
}

impl S3Scanner {
    /// Loads credentials and finds the region of each bucket to scan. For `s3://`
    /// (all buckets) this lists the buckets, which needs `s3:ListAllMyBuckets`.
    /// `region` overrides region detection.
    pub async fn connect(
        location: &S3Location,
        credentials: &CredentialSource,
        region: Option<&str>,
    ) -> Result<Self> {
        let loader = credentials.configure(aws_config::defaults(BehaviorVersion::latest()));
        let config = loader.load().await;
        check_credentials(&config).await?;

        let mut scanner = Self {
            location: location.clone(),
            targets: Vec::new(),
            skipped: Vec::new(),
            setup_requests: 0,
        };
        let mut clients = RegionalClients::new(&config);

        if !location.is_all_buckets() {
            let region = match region {
                Some(region) => region.to_owned(),
                None => bucket_region(&config, &location.bucket).await?,
            };
            scanner.add_target(&mut clients, location.bucket.clone(), region);
            return Ok(scanner);
        }

        let (buckets, requests) = list_buckets(&config).await?;
        scanner.setup_requests = requests;
        for (bucket, listed_region) in buckets {
            let region = match listed_region.or_else(|| region.map(str::to_owned)) {
                Some(region) => region,
                None => match bucket_region(&config, &bucket).await {
                    Ok(region) => region,
                    Err(error) => {
                        scanner.skip(bucket, &error);
                        continue;
                    }
                },
            };
            scanner.add_target(&mut clients, bucket, region);
        }
        Ok(scanner)
    }

    /// Storage prices for the scanned bucket(s), each at its own region's list prices.
    pub fn pricing(&self) -> S3Pricing {
        if self.location.is_all_buckets() {
            S3Pricing::for_buckets(
                self.targets
                    .iter()
                    .map(|target| (target.bucket.as_str(), target.region.as_str())),
            )
        } else {
            let region = self
                .targets
                .first()
                .map_or("", |target| target.region.as_str());
            S3Pricing::for_region(region)
        }
    }

    pub async fn scan(&self, options: &ScanOptions, sink: EntrySender) -> Result<ScanStats> {
        let requests = Arc::new(AtomicU64::new(self.setup_requests));
        let slots = Arc::new(Semaphore::new(options.concurrency.max(1)));
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let lister = |target: &Target, key_prefix: String| Lister {
            client: target.client.clone(),
            bucket: target.bucket.clone(),
            key_prefix,
            include_versions: options.include_versions,
            requests: Arc::clone(&requests),
            slots: Arc::clone(&slots),
            warnings: Arc::clone(&warnings),
            sink: sink.clone(),
        };
        let mut skipped = self.skipped.clone();

        if !self.location.is_all_buckets() {
            for target in &self.targets {
                let prefix = self.location.prefix.clone();
                scan_bucket(lister(target, String::new()), prefix, options.concurrency).await?;
            }
        } else {
            let mut tasks = JoinSet::new();
            for target in &self.targets {
                if tasks.len() >= BUCKETS_AT_ONCE
                    && let Some(finished) = tasks.join_next().await
                {
                    record_bucket_result(finished?, &mut skipped)?;
                }
                let lister = lister(target, format!("{}/", target.bucket));
                let bucket = target.bucket.clone();
                let concurrency = options.concurrency;
                tasks.spawn(async move {
                    (
                        bucket,
                        scan_bucket(lister, String::new(), concurrency).await,
                    )
                });
            }
            while let Some(finished) = tasks.join_next().await {
                record_bucket_result(finished?, &mut skipped)?;
            }
        }

        let warnings = warnings.lock().map(|list| list.clone()).unwrap_or_default();
        Ok(ScanStats {
            list_requests: requests.load(Ordering::Relaxed),
            skipped_buckets: skipped,
            warnings,
        })
    }

    fn add_target(&mut self, clients: &mut RegionalClients, bucket: String, region: String) {
        let client = clients.get(&region);
        self.targets.push(Target {
            bucket,
            region,
            client,
        });
    }

    fn skip(&mut self, bucket: String, error: &Error) {
        self.skipped.push(SkippedBucket {
            bucket,
            reason: error.to_string(),
        });
    }
}

/// In an all-buckets scan, a bucket that fails is skipped instead of ending the scan.
/// Cancellation still stops everything.
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

/// Lists everything under `prefix`: expands prefixes breadth-first until there are
/// enough to keep `concurrency` requests busy, then lists each one fully in parallel.
/// Incomplete multipart uploads under the prefix are listed last.
async fn scan_bucket(lister: Lister, prefix: String, concurrency: usize) -> Result<()> {
    let concurrency = concurrency.max(1);

    let mut prefixes = vec![prefix.clone()];
    for _ in 0..MAX_SPLIT_DEPTH {
        if prefixes.is_empty() || prefixes.len() >= concurrency {
            break;
        }
        let mut subprefixes = Vec::new();
        for prefix in prefixes {
            subprefixes.extend(lister.list(prefix, Some("/")).await?);
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
        tasks.spawn(async move { lister.list(prefix, None).await.map(drop) });
    }
    while let Some(finished) = tasks.join_next().await {
        finished??;
    }
    lister.list_incomplete_uploads(prefix).await
}

/// One S3 client per region, shared by every bucket in that region.
struct RegionalClients<'a> {
    config: &'a SdkConfig,
    clients: HashMap<String, Client>,
}

impl<'a> RegionalClients<'a> {
    fn new(config: &'a SdkConfig) -> Self {
        Self {
            config,
            clients: HashMap::new(),
        }
    }

    fn get(&mut self, region: &str) -> Client {
        self.clients
            .entry(region.to_owned())
            .or_insert_with(|| {
                let config = aws_sdk_s3::config::Builder::from(self.config)
                    .region(Region::new(region.to_owned()))
                    .build();
                Client::from_conf(config)
            })
            .clone()
    }
}

#[derive(Clone)]
struct Lister {
    client: Client,
    bucket: String,
    /// Prepended to every key; `bucket/` in an all-buckets scan so buckets become
    /// top-level folders.
    key_prefix: String,
    include_versions: bool,
    requests: Arc<AtomicU64>,
    /// Shared by every bucket in a scan: one slot per listing in progress.
    slots: Arc<Semaphore>,
    warnings: Arc<Mutex<Vec<String>>>,
    sink: EntrySender,
}

struct Upload {
    key: String,
    upload_id: String,
    storage_class: String,
    initiated: Option<u64>,
}

impl Lister {
    async fn list(&self, prefix: String, delimiter: Option<&str>) -> Result<Vec<String>> {
        let _slot = self.slots.acquire().await.map_err(|_| Error::Cancelled)?;
        if self.include_versions {
            self.list_versions(prefix, delimiter).await
        } else {
            self.list_objects(prefix, delimiter).await
        }
    }

    async fn list_objects(&self, prefix: String, delimiter: Option<&str>) -> Result<Vec<String>> {
        let mut pages = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .set_delimiter(delimiter.map(str::to_owned))
            .into_paginator()
            .send();

        let mut subprefixes = Vec::new();
        while let Some(page) = pages.next().await {
            let page =
                page.map_err(|error| request_error("ListObjectsV2", "s3:ListBucket", error))?;
            self.requests.fetch_add(1, Ordering::Relaxed);

            let entries = page
                .contents()
                .iter()
                .map(|object| Entry {
                    key: self.key(object.key()),
                    size: to_size(object.size()),
                    storage_class: storage_class(
                        object.storage_class().map(|class| class.as_str()),
                    ),
                    kind: EntryKind::Current,
                    last_modified: unix_seconds(object.last_modified()),
                })
                .collect();
            self.send(entries).await?;
            subprefixes.extend(prefix_names(page.common_prefixes()));
        }
        Ok(subprefixes)
    }

    async fn list_versions(&self, prefix: String, delimiter: Option<&str>) -> Result<Vec<String>> {
        let mut subprefixes = Vec::new();
        let mut key_marker = None;
        let mut version_marker = None;

        loop {
            let page = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .set_delimiter(delimiter.map(str::to_owned))
                .set_key_marker(key_marker.take())
                .set_version_id_marker(version_marker.take())
                .send()
                .await
                .map_err(|error| {
                    request_error("ListObjectVersions", "s3:ListBucketVersions", error)
                })?;
            self.requests.fetch_add(1, Ordering::Relaxed);

            let versions = page.versions().iter().map(|version| Entry {
                key: self.key(version.key()),
                size: to_size(version.size()),
                storage_class: storage_class(version.storage_class().map(|class| class.as_str())),
                kind: if version.is_latest().unwrap_or(false) {
                    EntryKind::Current
                } else {
                    EntryKind::Noncurrent
                },
                last_modified: unix_seconds(version.last_modified()),
            });
            let delete_markers = page.delete_markers().iter().map(|marker| Entry {
                key: self.key(marker.key()),
                size: 0,
                storage_class: DEFAULT_STORAGE_CLASS.to_owned(),
                kind: EntryKind::DeleteMarker,
                last_modified: unix_seconds(marker.last_modified()),
            });
            self.send(merge_by_key(versions, delete_markers)).await?;
            subprefixes.extend(prefix_names(page.common_prefixes()));

            if !page.is_truncated().unwrap_or(false) {
                return Ok(subprefixes);
            }
            key_marker = page.next_key_marker().map(str::to_owned);
            version_marker = page.next_version_id_marker().map(str::to_owned);
        }
    }

    /// Sends every incomplete multipart upload under `prefix` as an entry sized by its
    /// uploaded parts. Missing permissions become warnings, not errors.
    async fn list_incomplete_uploads(&self, prefix: String) -> Result<()> {
        let uploads = match self.find_uploads(prefix).await {
            Ok(uploads) => uploads,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) => {
                self.warn(format!(
                    "{}: incomplete uploads not checked. {error}",
                    self.bucket
                ));
                return Ok(());
            }
        };

        let mut tasks = JoinSet::new();
        for upload in uploads {
            let lister = self.clone();
            tasks.spawn(async move {
                let size = lister.uploaded_bytes(&upload).await;
                (upload, size)
            });
        }

        let mut entries = Vec::new();
        let mut unknown_sizes = None;
        while let Some(finished) = tasks.join_next().await {
            let (upload, size) = finished?;
            let size = match size {
                Ok(size) => size,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    unknown_sizes.get_or_insert(error);
                    0
                }
            };
            entries.push(Entry {
                key: self.key(Some(&upload.key)),
                size,
                storage_class: upload.storage_class,
                kind: EntryKind::IncompleteUpload,
                last_modified: upload.initiated,
            });
        }
        if let Some(error) = unknown_sizes {
            self.warn(format!(
                "{}: sizes of incomplete uploads unknown. {error}",
                self.bucket
            ));
        }
        for batch in entries.chunks(1000) {
            self.send(batch.to_vec()).await?;
        }
        Ok(())
    }

    async fn find_uploads(&self, prefix: String) -> Result<Vec<Upload>> {
        let _slot = self.slots.acquire().await.map_err(|_| Error::Cancelled)?;
        let mut uploads = Vec::new();
        let mut key_marker = None;
        let mut upload_id_marker = None;
        loop {
            let page = self
                .client
                .list_multipart_uploads()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .set_key_marker(key_marker.take())
                .set_upload_id_marker(upload_id_marker.take())
                .send()
                .await
                .map_err(|error| {
                    request_error(
                        "ListMultipartUploads",
                        "s3:ListBucketMultipartUploads",
                        error,
                    )
                })?;
            self.requests.fetch_add(1, Ordering::Relaxed);

            uploads.extend(page.uploads().iter().filter_map(|upload| {
                Some(Upload {
                    key: upload.key()?.to_owned(),
                    upload_id: upload.upload_id()?.to_owned(),
                    storage_class: storage_class(
                        upload.storage_class().map(|class| class.as_str()),
                    ),
                    initiated: unix_seconds(upload.initiated()),
                })
            }));

            if !page.is_truncated().unwrap_or(false) {
                return Ok(uploads);
            }
            key_marker = page.next_key_marker().map(str::to_owned);
            upload_id_marker = page.next_upload_id_marker().map(str::to_owned);
        }
    }

    /// The billed size of an upload: the sum of the parts uploaded so far.
    async fn uploaded_bytes(&self, upload: &Upload) -> Result<u64> {
        let _slot = self.slots.acquire().await.map_err(|_| Error::Cancelled)?;
        let mut pages = self
            .client
            .list_parts()
            .bucket(&self.bucket)
            .key(&upload.key)
            .upload_id(&upload.upload_id)
            .into_paginator()
            .send();
        let mut bytes = 0;
        while let Some(page) = pages.next().await {
            let page = page.map_err(|error| {
                request_error("ListParts", "s3:ListMultipartUploadParts", error)
            })?;
            self.requests.fetch_add(1, Ordering::Relaxed);
            bytes += page
                .parts()
                .iter()
                .map(|part| to_size(part.size()))
                .sum::<u64>();
        }
        Ok(bytes)
    }

    fn warn(&self, warning: String) {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(warning);
        }
    }

    fn key(&self, key: Option<&str>) -> String {
        format!("{}{}", self.key_prefix, key.unwrap_or_default())
    }

    async fn send(&self, entries: Vec<Entry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.sink.send(entries).await.map_err(|_| Error::Cancelled)
    }
}

async fn check_credentials(config: &SdkConfig) -> Result<()> {
    let provider = config
        .credentials_provider()
        .ok_or_else(|| Error::Credentials("no credentials provider configured".to_owned()))?;
    provider
        .provide_credentials()
        .await
        .map_err(|error| Error::Credentials(DisplayErrorContext(error).to_string()))?;
    Ok(())
}

/// Every bucket the credentials can list, with its region when S3 reports it,
/// plus the number of requests used.
async fn list_buckets(config: &SdkConfig) -> Result<(Vec<(String, Option<String>)>, u64)> {
    let lookup_config = aws_sdk_s3::config::Builder::from(config)
        .region(Region::from_static("us-east-1"))
        .build();
    let client = Client::from_conf(lookup_config);

    let mut pages = client
        .list_buckets()
        .max_buckets(1000)
        .into_paginator()
        .send();
    let mut buckets = Vec::new();
    let mut requests = 0;
    while let Some(page) = pages.next().await {
        let page =
            page.map_err(|error| request_error("ListBuckets", "s3:ListAllMyBuckets", error))?;
        requests += 1;
        buckets.extend(page.buckets().iter().filter_map(|bucket| {
            let name = bucket.name()?.to_owned();
            Some((name, bucket.bucket_region().map(str::to_owned)))
        }));
    }
    Ok((buckets, requests))
}

async fn bucket_region(config: &SdkConfig, bucket: &str) -> Result<String> {
    let lookup_config = aws_sdk_s3::config::Builder::from(config)
        .region(Region::from_static("us-east-1"))
        .build();
    let client = Client::from_conf(lookup_config);

    match client.head_bucket().bucket(bucket).send().await {
        Ok(output) => Ok(output.bucket_region().unwrap_or("us-east-1").to_owned()),
        Err(error) => {
            let redirected_region = error
                .raw_response()
                .and_then(|response| response.headers().get("x-amz-bucket-region"))
                .map(str::to_owned);
            redirected_region.ok_or_else(|| request_error("HeadBucket", "s3:ListBucket", error))
        }
    }
}

fn request_error(
    operation: &'static str,
    permission: &'static str,
    error: impl std::error::Error,
) -> Error {
    Error::Request {
        operation,
        permission,
        message: DisplayErrorContext(error).to_string(),
    }
}

/// S3 returns a page's versions and delete markers as two lists, each sorted by key.
/// Merging them keeps every version of a key together, which is how the tree
/// recognizes them as one object.
fn merge_by_key(
    versions: impl Iterator<Item = Entry>,
    delete_markers: impl Iterator<Item = Entry>,
) -> Vec<Entry> {
    let mut versions = versions.peekable();
    let mut delete_markers = delete_markers.peekable();
    let mut merged = Vec::new();
    loop {
        let next = match (versions.peek(), delete_markers.peek()) {
            (Some(version), Some(marker)) if marker.key < version.key => delete_markers.next(),
            (Some(_), _) => versions.next(),
            (None, _) => delete_markers.next(),
        };
        match next {
            Some(entry) => merged.push(entry),
            None => return merged,
        }
    }
}

fn prefix_names(prefixes: &[CommonPrefix]) -> impl Iterator<Item = String> + '_ {
    prefixes
        .iter()
        .filter_map(|prefix| prefix.prefix())
        .map(str::to_owned)
}

fn to_size(size: Option<i64>) -> u64 {
    size.and_then(|size| u64::try_from(size).ok()).unwrap_or(0)
}

/// Seconds since the Unix epoch; `None` when missing or before 1970.
fn unix_seconds(time: Option<&DateTime>) -> Option<u64> {
    time.and_then(|time| u64::try_from(time.secs()).ok())
}

fn storage_class(class: Option<&str>) -> String {
    class.unwrap_or(DEFAULT_STORAGE_CLASS).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, kind: EntryKind) -> Entry {
        Entry {
            key: key.to_owned(),
            size: 1,
            storage_class: DEFAULT_STORAGE_CLASS.to_owned(),
            kind,
            last_modified: None,
        }
    }

    #[test]
    fn delete_markers_join_their_key() {
        let versions = [
            entry("a", EntryKind::Noncurrent),
            entry("c", EntryKind::Current),
        ];
        let markers = [
            entry("a", EntryKind::DeleteMarker),
            entry("b", EntryKind::DeleteMarker),
        ];

        let keys: Vec<_> = merge_by_key(versions.into_iter(), markers.into_iter())
            .into_iter()
            .map(|entry| (entry.key, entry.kind))
            .collect();

        assert_eq!(
            keys,
            [
                ("a".to_owned(), EntryKind::Noncurrent),
                ("a".to_owned(), EntryKind::DeleteMarker),
                ("b".to_owned(), EntryKind::DeleteMarker),
                ("c".to_owned(), EntryKind::Current),
            ]
        );
    }
}
