mod location;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::ProvideCredentials;
use aws_sdk_s3::error::DisplayErrorContext;
use aws_sdk_s3::types::CommonPrefix;
use clouddirstat_core::{Entry, EntryKind};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;

pub use location::S3Location;

use crate::{Error, Result};

const LIST_PRICE_PER_1000_USD: f64 = 0.005;
const DEFAULT_STORAGE_CLASS: &str = "STANDARD";
const MAX_SPLIT_DEPTH: usize = 3;

pub type EntrySender = Sender<Vec<Entry>>;

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub include_versions: bool,
    pub concurrency: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ScanStats {
    pub list_requests: u64,
}

impl ScanStats {
    pub fn estimated_cost_usd(&self) -> f64 {
        self.list_requests as f64 / 1000.0 * LIST_PRICE_PER_1000_USD
    }
}

pub struct S3Scanner {
    client: Client,
}

impl S3Scanner {
    pub async fn connect(
        bucket: &str,
        profile: Option<&str>,
        region: Option<&str>,
    ) -> Result<Self> {
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(profile) = profile {
            loader = loader.profile_name(profile);
        }
        let config = loader.load().await;
        check_credentials(&config).await?;

        let region = match region {
            Some(region) => region.to_owned(),
            None => bucket_region(&config, bucket).await?,
        };

        let s3_config = aws_sdk_s3::config::Builder::from(&config)
            .region(Region::new(region))
            .build();
        Ok(Self {
            client: Client::from_conf(s3_config),
        })
    }

    pub async fn scan(
        &self,
        location: &S3Location,
        options: &ScanOptions,
        sink: EntrySender,
    ) -> Result<ScanStats> {
        let lister = Lister {
            client: self.client.clone(),
            bucket: location.bucket.clone(),
            include_versions: options.include_versions,
            requests: Arc::default(),
            sink,
        };
        let concurrency = options.concurrency.max(1);

        let mut prefixes = vec![location.prefix.clone()];
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

        Ok(ScanStats {
            list_requests: lister.requests.load(Ordering::Relaxed),
        })
    }
}

#[derive(Clone)]
struct Lister {
    client: Client,
    bucket: String,
    include_versions: bool,
    requests: Arc<AtomicU64>,
    sink: EntrySender,
}

impl Lister {
    async fn list(&self, prefix: String, delimiter: Option<&str>) -> Result<Vec<String>> {
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
                    key: object.key().unwrap_or_default().to_owned(),
                    size: to_size(object.size()),
                    storage_class: storage_class(
                        object.storage_class().map(|class| class.as_str()),
                    ),
                    kind: EntryKind::Current,
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
                key: version.key().unwrap_or_default().to_owned(),
                size: to_size(version.size()),
                storage_class: storage_class(version.storage_class().map(|class| class.as_str())),
                kind: if version.is_latest().unwrap_or(false) {
                    EntryKind::Current
                } else {
                    EntryKind::Noncurrent
                },
            });
            let delete_markers = page.delete_markers().iter().map(|marker| Entry {
                key: marker.key().unwrap_or_default().to_owned(),
                size: 0,
                storage_class: DEFAULT_STORAGE_CLASS.to_owned(),
                kind: EntryKind::DeleteMarker,
            });
            self.send(versions.chain(delete_markers).collect()).await?;
            subprefixes.extend(prefix_names(page.common_prefixes()));

            if !page.is_truncated().unwrap_or(false) {
                return Ok(subprefixes);
            }
            key_marker = page.next_key_marker().map(str::to_owned);
            version_marker = page.next_version_id_marker().map(str::to_owned);
        }
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

fn prefix_names(prefixes: &[CommonPrefix]) -> impl Iterator<Item = String> + '_ {
    prefixes
        .iter()
        .filter_map(|prefix| prefix.prefix())
        .map(str::to_owned)
}

fn to_size(size: Option<i64>) -> u64 {
    size.and_then(|size| u64::try_from(size).ok()).unwrap_or(0)
}

fn storage_class(class: Option<&str>) -> String {
    class.unwrap_or(DEFAULT_STORAGE_CLASS).to_owned()
}
