//! Google Cloud Storage, through its JSON API.

mod auth;
mod location;
mod prices;
mod pricing;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clouddirstat_core::{Entry, EntryKind};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub use auth::GcsCredentials;
pub use location::GcsLocation;
pub use pricing::{BucketPlacement, GcsPricing};

use self::auth::TokenSource;
use crate::http::{Http, Response, encode};
use crate::time::rfc3339_seconds;
use crate::{EntrySender, Error, Result, ScanOptions, ScanStats, SkippedBucket};

const API: &str = "https://storage.googleapis.com/storage/v1";
const DEFAULT_STORAGE_CLASS: &str = "STANDARD";
const MAX_SPLIT_DEPTH: usize = 3;
/// Buckets in progress at the same time in an all-buckets scan.
const BUCKETS_AT_ONCE: usize = 16;
/// Only the fields the tree needs, which keeps listing responses small.
const OBJECT_FIELDS: &str =
    "nextPageToken,prefixes,items(name,size,storageClass,updated,timeDeleted)";
const BUCKET_FIELDS: &str = "name,location,locationType,storageClass,customPlacementConfig";

/// Lists one bucket, or every bucket of a project.
pub struct GcsScanner {
    http: Http,
    auth: Arc<TokenSource>,
    location: GcsLocation,
    targets: Vec<Target>,
    setup_requests: u64,
    /// Problems found while connecting that do not stop the scan.
    warnings: Vec<String>,
}

struct Target {
    bucket: String,
    placement: BucketPlacement,
    /// New objects get this class; it also sets the price of listing.
    default_class: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BucketResource {
    name: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    location_type: String,
    storage_class: Option<String>,
    custom_placement_config: Option<CustomPlacement>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomPlacement {
    #[serde(default)]
    data_locations: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BucketPage {
    #[serde(default)]
    items: Vec<BucketResource>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectPage {
    #[serde(default)]
    items: Vec<ObjectResource>,
    #[serde(default)]
    prefixes: Vec<String>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectResource {
    name: String,
    size: Option<String>,
    storage_class: Option<String>,
    updated: Option<String>,
    /// Set on noncurrent versions of a versioned bucket.
    time_deleted: Option<String>,
}

impl BucketResource {
    fn into_target(self) -> Target {
        Target {
            placement: BucketPlacement {
                location: self.location,
                location_type: self.location_type,
                data_locations: self
                    .custom_placement_config
                    .map(|config| config.data_locations)
                    .unwrap_or_default(),
            },
            default_class: self
                .storage_class
                .unwrap_or_else(|| DEFAULT_STORAGE_CLASS.to_owned()),
            bucket: self.name,
        }
    }
}

impl GcsScanner {
    /// Signs in and reads the bucket's location (or lists the project's buckets for
    /// `gs://`, which needs `storage.buckets.list`).
    pub async fn connect(location: &GcsLocation, credentials: &GcsCredentials) -> Result<Self> {
        let http = Http::new()?;
        let auth = Arc::new(TokenSource::new(credentials, http.clone())?);
        // Fail now, with a clear message, if the credentials do not work.
        auth.token().await?;

        let mut scanner = Self {
            http,
            auth,
            location: location.clone(),
            targets: Vec::new(),
            setup_requests: 0,
            warnings: Vec::new(),
        };
        if location.is_all_buckets() {
            let project = project(credentials, &scanner.auth)?;
            scanner.targets = scanner.list_buckets(&project).await?;
        } else {
            let target = scanner.bucket(&location.bucket).await?;
            scanner.targets.push(target);
        }
        Ok(scanner)
    }

    /// Storage prices for the scanned bucket(s), each at its own location's prices.
    pub fn pricing(&self) -> GcsPricing {
        if self.location.is_all_buckets() {
            GcsPricing::for_buckets(
                self.targets
                    .iter()
                    .map(|target| (target.bucket.as_str(), &target.placement)),
            )
        } else {
            let placement = self
                .targets
                .first()
                .map(|target| target.placement.clone())
                .unwrap_or_default();
            GcsPricing::for_bucket(&placement)
        }
    }

    pub async fn scan(&self, options: &ScanOptions, sink: EntrySender) -> Result<ScanStats> {
        let requests = Arc::new(AtomicU64::new(self.setup_requests));
        let cost = Arc::new(AtomicU64::new(0));
        let slots = Arc::new(Semaphore::new(options.concurrency.max(1)));
        let warnings = Arc::new(Mutex::new(self.warnings.clone()));
        let lister = |target: &Target, key_prefix: String| Lister {
            http: self.http.clone(),
            auth: Arc::clone(&self.auth),
            bucket: target.bucket.clone(),
            default_class: target.default_class.clone(),
            key_prefix,
            include_versions: options.include_versions,
            list_price_per_1000: GcsPricing::list_price_per_1000(&target.default_class),
            requests: Arc::clone(&requests),
            cost_nanodollars: Arc::clone(&cost),
            slots: Arc::clone(&slots),
            warnings: Arc::clone(&warnings),
            sink: sink.clone(),
        };

        let mut skipped = Vec::new();
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

        let list_requests = requests.load(Ordering::Relaxed);
        let cost_usd = cost.load(Ordering::Relaxed) as f64 / 1e9;
        let warnings = warnings.lock().map(|list| list.clone()).unwrap_or_default();
        Ok(ScanStats {
            list_requests,
            list_price_per_1000_usd: if list_requests == 0 {
                GcsPricing::list_price_per_1000(DEFAULT_STORAGE_CLASS)
            } else {
                cost_usd * 1000.0 / list_requests as f64
            },
            skipped_buckets: skipped,
            warnings,
        })
    }

    /// One bucket's location and default class. Without `storage.buckets.get` the scan
    /// still runs, priced at us-central1 rates, with a warning.
    async fn bucket(&mut self, bucket: &str) -> Result<Target> {
        let url = format!(
            "{API}/b/{}?fields={}",
            encode(bucket),
            encode(BUCKET_FIELDS)
        );
        let response = self.get(&url).await?;
        self.setup_requests += 1;
        match response.status.as_u16() {
            200 => Ok(parse::<BucketResource>(&response)?.into_target()),
            404 => Err(Error::NoSuchBucket(bucket.to_owned())),
            401 => Err(rejected(&response)),
            _ => {
                self.warnings.push(format!(
                    "{bucket}: location unknown, so priced at us-central1 rates. {}",
                    api_error("buckets.get", "storage.buckets.get", &response)
                ));
                Ok(Target {
                    bucket: bucket.to_owned(),
                    placement: BucketPlacement::default(),
                    default_class: DEFAULT_STORAGE_CLASS.to_owned(),
                })
            }
        }
    }

    async fn list_buckets(&mut self, project: &str) -> Result<Vec<Target>> {
        let mut targets = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "{API}/b?project={}&maxResults=1000&fields={}",
                encode(project),
                encode(&format!("nextPageToken,items({BUCKET_FIELDS})"))
            );
            if let Some(token) = &page_token {
                url.push_str(&format!("&pageToken={}", encode(token)));
            }
            let response = self.get(&url).await?;
            self.setup_requests += 1;
            if !response.status.is_success() {
                return Err(match response.status.as_u16() {
                    401 => rejected(&response),
                    _ => api_error("buckets.list", "storage.buckets.list", &response),
                });
            }
            let page: BucketPage = parse(&response)?;
            targets.extend(page.items.into_iter().map(BucketResource::into_target));
            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => return Ok(targets),
            }
        }
    }

    async fn get(&self, url: &str) -> Result<Response> {
        let token = self.auth.token().await?;
        let authorization = format!("Bearer {token}");
        self.http
            .get(url, &[("authorization", &authorization)])
            .await
    }
}

/// The project for `gs://`: the one given, then GOOGLE_CLOUD_PROJECT or
/// CLOUDSDK_CORE_PROJECT, then the credentials file's, then gcloud's configuration.
fn project(credentials: &GcsCredentials, auth: &TokenSource) -> Result<String> {
    let from_env = || {
        ["GOOGLE_CLOUD_PROJECT", "CLOUDSDK_CORE_PROJECT"]
            .into_iter()
            .find_map(|name| std::env::var(name).ok())
    };
    credentials
        .project
        .clone()
        .filter(|project| !project.trim().is_empty())
        .or_else(from_env)
        .or_else(|| auth.project.clone())
        .or_else(auth::gcloud_project)
        .ok_or_else(|| {
            Error::Credentials(
                "gs:// lists the buckets of a Google Cloud project, but no project is set: \
                 enter one, or run `gcloud config set project PROJECT_ID`"
                    .to_owned(),
            )
        })
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

/// Lists everything under `prefix`: expands prefixes breadth-first until there are
/// enough to keep `concurrency` requests busy, then lists each one fully in parallel.
/// With versions, soft-deleted objects (billed until their retention ends) come last.
async fn scan_bucket(lister: Lister, prefix: String, concurrency: usize) -> Result<()> {
    let concurrency = concurrency.max(1);

    let mut prefixes = vec![prefix.clone()];
    for _ in 0..MAX_SPLIT_DEPTH {
        if prefixes.is_empty() || prefixes.len() >= concurrency {
            break;
        }
        let mut subprefixes = Vec::new();
        for prefix in prefixes {
            subprefixes.extend(lister.list(prefix, Some("/"), false).await?);
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
        tasks.spawn(async move { lister.list(prefix, None, false).await.map(drop) });
    }
    while let Some(finished) = tasks.join_next().await {
        finished??;
    }

    if lister.include_versions {
        match lister.list(prefix, None, true).await {
            Ok(_) => {}
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) => lister.warn(format!(
                "{}: soft-deleted objects not checked. {error}",
                lister.bucket
            )),
        }
    }
    Ok(())
}

#[derive(Clone)]
struct Lister {
    http: Http,
    auth: Arc<TokenSource>,
    bucket: String,
    default_class: String,
    /// Prepended to every key; `bucket/` in an all-buckets scan.
    key_prefix: String,
    include_versions: bool,
    list_price_per_1000: f64,
    requests: Arc<AtomicU64>,
    cost_nanodollars: Arc<AtomicU64>,
    slots: Arc<Semaphore>,
    warnings: Arc<Mutex<Vec<String>>>,
    sink: EntrySender,
}

impl Lister {
    /// Lists objects under `prefix`, sending them to the tree, and returns the
    /// subprefixes when `delimiter` is given. `soft_deleted` lists soft-deleted objects
    /// instead of live ones (and their old versions).
    async fn list(
        &self,
        prefix: String,
        delimiter: Option<&str>,
        soft_deleted: bool,
    ) -> Result<Vec<String>> {
        let _slot = self.slots.acquire().await.map_err(|_| Error::Cancelled)?;
        let mut subprefixes = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut url = format!(
                "{API}/b/{}/o?prefix={}&maxResults=1000&fields={}",
                encode(&self.bucket),
                encode(&prefix),
                encode(OBJECT_FIELDS)
            );
            if let Some(delimiter) = delimiter {
                url.push_str(&format!("&delimiter={}", encode(delimiter)));
            }
            if soft_deleted {
                url.push_str("&softDeleted=true");
            } else if self.include_versions {
                url.push_str("&versions=true");
            }
            if let Some(token) = &page_token {
                url.push_str(&format!("&pageToken={}", encode(token)));
            }

            let token = self.auth.token().await?;
            let authorization = format!("Bearer {token}");
            let response = self
                .http
                .get(&url, &[("authorization", &authorization)])
                .await?;
            self.count_request();
            match response.status.as_u16() {
                200 => {}
                401 => return Err(rejected(&response)),
                404 => return Err(Error::NoSuchBucket(self.bucket.clone())),
                _ => return Err(api_error("objects.list", "storage.objects.list", &response)),
            }

            let page: ObjectPage = parse(&response)?;
            let entries = page
                .items
                .into_iter()
                .map(|object| self.entry(object, soft_deleted))
                .collect();
            self.sink
                .send(entries)
                .await
                .map_err(|_| Error::Cancelled)?;
            subprefixes.extend(page.prefixes);
            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => return Ok(subprefixes),
            }
        }
    }

    fn entry(&self, object: ObjectResource, soft_deleted: bool) -> Entry {
        let noncurrent = soft_deleted || object.time_deleted.is_some();
        Entry {
            key: format!("{}{}", self.key_prefix, object.name),
            size: object.size.and_then(|size| size.parse().ok()).unwrap_or(0),
            storage_class: object
                .storage_class
                .unwrap_or_else(|| self.default_class.clone()),
            kind: if noncurrent {
                EntryKind::Noncurrent
            } else {
                EntryKind::Current
            },
            last_modified: object.updated.as_deref().and_then(rfc3339_seconds),
        }
    }

    /// Listing soft-deleted objects is a cheaper Class B operation, but is counted at
    /// the Class A price with the rest, so the scan cost errs high.
    fn count_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        let nanodollars = (self.list_price_per_1000 * 1e6).round() as u64;
        self.cost_nanodollars
            .fetch_add(nanodollars, Ordering::Relaxed);
    }

    fn warn(&self, warning: String) {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(warning);
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(response: &Response) -> Result<T> {
    serde_json::from_slice(&response.body)
        .map_err(|error| Error::Network(format!("unexpected Cloud Storage response: {error}")))
}

/// Google rejected the token: expired, revoked, or for another account.
fn rejected(response: &Response) -> Error {
    Error::Credentials(format!(
        "Google rejected the credentials ({}). Run `gcloud auth application-default login` \
         to sign in again, or paste a fresh access token.",
        error_message(response)
    ))
}

fn api_error(operation: &'static str, permission: &'static str, response: &Response) -> Error {
    Error::Request {
        operation,
        permission,
        message: error_message(response),
    }
}

/// The message of a JSON API error (`{"error": {"message": ...}}`), or the HTTP status.
fn error_message(response: &Response) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: Detail,
    }
    #[derive(Deserialize)]
    struct Detail {
        message: String,
    }
    match serde_json::from_slice::<Body>(&response.body) {
        Ok(body) => format!("HTTP {}: {}", response.status.as_u16(), body.error.message),
        Err(_) => format!("HTTP {}", response.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object_pages() {
        let page: ObjectPage = serde_json::from_str(
            r#"{
                "prefixes": ["logs/"],
                "items": [
                    {"name": "a.txt", "size": "42", "storageClass": "NEARLINE",
                     "updated": "2026-09-29T00:00:00.000Z"},
                    {"name": "a.txt", "size": "7", "storageClass": "STANDARD",
                     "updated": "2026-01-01T00:00:00Z", "timeDeleted": "2026-09-29T00:00:00Z"}
                ],
                "nextPageToken": "abc"
            }"#,
        )
        .unwrap();
        assert_eq!(page.prefixes, ["logs/"]);
        assert_eq!(page.next_page_token.as_deref(), Some("abc"));
        assert_eq!(page.items.len(), 2);
        assert!(page.items[1].time_deleted.is_some());

        let empty: ObjectPage = serde_json::from_str("{}").unwrap();
        assert!(empty.items.is_empty() && empty.next_page_token.is_none());
    }

    #[test]
    fn reads_bucket_placement() {
        let bucket: BucketResource = serde_json::from_str(
            r#"{"name": "b", "location": "US", "locationType": "dual-region",
                "storageClass": "COLDLINE",
                "customPlacementConfig": {"dataLocations": ["US-CENTRAL1", "US-EAST1"]}}"#,
        )
        .unwrap();
        let target = bucket.into_target();
        assert_eq!(target.default_class, "COLDLINE");
        assert_eq!(target.placement.location_type, "dual-region");
        assert_eq!(target.placement.data_locations, ["US-CENTRAL1", "US-EAST1"]);
    }
}
