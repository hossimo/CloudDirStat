//! Azure Blob Storage, through the Blob REST API; storage accounts and their regions
//! come from Azure Resource Manager.

mod auth;
mod location;
mod prices;
mod pricing;

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clouddirstat_core::{Entry, EntryKind};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub use auth::AzureCredentials;
pub use location::AzureLocation;
pub use pricing::{AccountPlacement, AzurePricing};

use self::auth::{BlobAuth, CliTokens, MANAGEMENT, STORAGE};
use crate::http::{Http, Response, encode};
use crate::scanner::class_name;
use crate::time::http_date_seconds;
use crate::xml::{XmlEvent, walk};
use crate::{EntrySender, Error, Result, ScanOptions, ScanStats, SkippedBucket};

const API_VERSION: &str = "2023-11-03";
const ARM: &str = "https://management.azure.com";
const DEFAULT_TIER: &str = "Hot";
/// The access tiers Azure lists blobs in.
const TIERS: &[&str] = &["Hot", "Cool", "Cold", "Archive", "Premium"];
const MAX_SPLIT_DEPTH: usize = 3;
/// Containers in progress at the same time when scanning more than one.
const CONTAINERS_AT_ONCE: usize = 16;
/// Subscriptions (or accounts) looked up at the same time while connecting.
const LOOKUPS_AT_ONCE: usize = 16;

/// Lists one container, every container of an account, or every account.
pub struct AzureScanner {
    http: Http,
    /// For Azure Resource Manager, which only the Azure CLI signs in to.
    tokens: Arc<CliTokens>,
    auth: Arc<BlobAuth>,
    location: AzureLocation,
    accounts: Vec<Account>,
    targets: Vec<Target>,
    setup_requests: AtomicU64,
    skipped: Vec<SkippedBucket>,
    warnings: Vec<String>,
}

struct Account {
    name: String,
    placement: AccountPlacement,
}

struct Target {
    account: String,
    container: String,
    /// Prepended to every key: `account/container/`, `container/`, or nothing,
    /// depending on how much the location covers.
    key_prefix: String,
    list_price_per_1000: f64,
}

#[derive(Deserialize)]
struct ArmList<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
    #[serde(rename = "nextLink")]
    next_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    subscription_id: String,
}

#[derive(Deserialize)]
struct StorageAccount {
    name: String,
    #[serde(default)]
    location: String,
    sku: Option<Sku>,
}

#[derive(Deserialize)]
struct Sku {
    name: String,
}

impl AzureScanner {
    /// Signs in and finds the containers to list, and each account's region and
    /// redundancy for prices (from Azure Resource Manager, when the user may read it).
    pub async fn connect(location: &AzureLocation, credentials: &AzureCredentials) -> Result<Self> {
        let tokens = Arc::new(CliTokens::default());
        let auth = credentials.blob_auth(&location.account, &tokens)?;
        let mut scanner = Self {
            http: Http::new()?,
            tokens,
            auth: Arc::new(auth),
            location: location.clone(),
            accounts: Vec::new(),
            targets: Vec::new(),
            setup_requests: AtomicU64::new(0),
            skipped: Vec::new(),
            warnings: Vec::new(),
        };
        if scanner.auth.uses_cli() {
            // Fail now, with a clear message, if the Azure CLI cannot sign in.
            scanner.tokens.token(STORAGE).await?;
        }

        if location.is_all_accounts() {
            // A connection string covers only the account it names.
            scanner.accounts = match scanner.auth.account().map(str::to_owned) {
                Some(name) => {
                    let placement = scanner.placement(&name).await;
                    vec![Account { name, placement }]
                }
                None => scanner.resource_manager_accounts().await?,
            };
            let names: Vec<String> = scanner.accounts.iter().map(|a| a.name.clone()).collect();
            let this = &scanner;
            let listed: Vec<(String, Result<Vec<String>>)> = stream::iter(names)
                .map(|account| async move {
                    let containers = this.containers(&account).await;
                    (account, containers)
                })
                .buffered(LOOKUPS_AT_ONCE)
                .collect()
                .await;
            for (account, containers) in listed {
                match containers {
                    Ok(containers) => {
                        for container in containers {
                            let key_prefix = format!("{account}/{container}/");
                            scanner.add_target(&account, container, key_prefix);
                        }
                    }
                    Err(error) => scanner.skipped.push(SkippedBucket {
                        bucket: account,
                        reason: error.to_string(),
                    }),
                }
            }
            return Ok(scanner);
        }

        let placement = scanner.placement(&location.account).await;
        scanner.accounts.push(Account {
            name: location.account.clone(),
            placement,
        });
        if location.is_all_containers() {
            for container in scanner.containers(&location.account).await? {
                let key_prefix = format!("{container}/");
                scanner.add_target(&location.account, container, key_prefix);
            }
        } else {
            let container = location.container.clone();
            scanner.add_target(&location.account, container, String::new());
        }
        Ok(scanner)
    }

    /// Storage prices for the scanned account(s).
    pub fn pricing(&self) -> AzurePricing {
        if self.location.is_all_accounts() {
            AzurePricing::for_accounts(
                self.accounts
                    .iter()
                    .map(|account| (account.name.as_str(), &account.placement)),
            )
        } else {
            let placement = self
                .accounts
                .first()
                .map(|account| account.placement.clone())
                .unwrap_or_default();
            AzurePricing::for_account(&placement)
        }
    }

    pub async fn scan(&self, options: &ScanOptions, sink: EntrySender) -> Result<ScanStats> {
        let requests = Arc::new(AtomicU64::new(self.setup_requests.load(Ordering::Relaxed)));
        let cost = Arc::new(AtomicU64::new(0));
        let slots = Arc::new(Semaphore::new(options.concurrency.max(1)));
        let warnings = Arc::new(Mutex::new(self.warnings.clone()));
        let lister = |target: &Target| Lister {
            http: self.http.clone(),
            auth: Arc::clone(&self.auth),
            account: target.account.clone(),
            container: target.container.clone(),
            key_prefix: target.key_prefix.clone(),
            include_versions: options.include_versions,
            versions_unsupported: Arc::new(AtomicBool::new(false)),
            list_price_per_1000: target.list_price_per_1000,
            requests: Arc::clone(&requests),
            cost_nanodollars: Arc::clone(&cost),
            slots: Arc::clone(&slots),
            warnings: Arc::clone(&warnings),
            sink: sink.clone(),
        };

        let mut skipped = self.skipped.clone();
        if self.targets.len() == 1 && !self.location.container.is_empty() {
            let prefix = self.location.prefix.clone();
            scan_container(lister(&self.targets[0]), prefix, options.concurrency).await?;
        } else {
            let mut tasks = JoinSet::new();
            for target in &self.targets {
                if tasks.len() >= CONTAINERS_AT_ONCE
                    && let Some(finished) = tasks.join_next().await
                {
                    record_result(finished?, &mut skipped)?;
                }
                let lister = lister(target);
                let name = target.key_prefix.trim_end_matches('/').to_owned();
                let concurrency = options.concurrency;
                tasks.spawn(async move {
                    (
                        name,
                        scan_container(lister, String::new(), concurrency).await,
                    )
                });
            }
            while let Some(finished) = tasks.join_next().await {
                record_result(finished?, &mut skipped)?;
            }
        }

        let list_requests = requests.load(Ordering::Relaxed);
        let cost_usd = cost.load(Ordering::Relaxed) as f64 / 1e9;
        let warnings = warnings.lock().map(|list| list.clone()).unwrap_or_default();
        Ok(ScanStats {
            list_requests,
            list_price_per_1000_usd: if list_requests == 0 {
                AzurePricing::list_price_per_1000(&AccountPlacement::default())
            } else {
                cost_usd * 1000.0 / list_requests as f64
            },
            skipped_buckets: skipped,
            warnings,
        })
    }

    fn add_target(&mut self, account: &str, container: String, key_prefix: String) {
        let placement = self
            .accounts
            .iter()
            .find(|candidate| candidate.name == account)
            .map(|account| account.placement.clone())
            .unwrap_or_default();
        self.targets.push(Target {
            account: account.to_owned(),
            container,
            key_prefix,
            list_price_per_1000: AzurePricing::list_price_per_1000(&placement),
        });
    }

    /// The account's region and redundancy. Finding them is optional: without them the
    /// scan uses eastus LRS prices, with a warning. With the Azure CLI they come from
    /// Azure Resource Manager. A SAS token or account key never reaches beyond the
    /// account, so only its redundancy is found, from the Blob service.
    async fn placement(&mut self, account: &str) -> AccountPlacement {
        if !self.auth.uses_cli() {
            return self.blob_service_placement(account).await;
        }
        match self.resource_manager_accounts().await {
            Ok(accounts) => match accounts.into_iter().find(|found| found.name == account) {
                Some(found) => found.placement,
                None => {
                    self.warnings.push(format!(
                        "{account}: not found in your subscriptions, so priced at eastus LRS \
                         rates"
                    ));
                    AccountPlacement::default()
                }
            },
            Err(error) => {
                self.warnings.push(format!(
                    "{account}: region unknown, so priced at eastus LRS rates. {error}"
                ));
                AccountPlacement::default()
            }
        }
    }

    /// The account's redundancy from Get Account Information, a metadata read that a SAS
    /// or account key allows. The region stays unknown, so it is priced as eastus.
    async fn blob_service_placement(&mut self, account: &str) -> AccountPlacement {
        let url = format!(
            "https://{account}.blob.core.windows.net/{}?restype=account&comp=properties",
            encode(&self.location.container)
        );
        let sku = match self.auth.get(&self.http, &url, API_VERSION).await {
            Ok(response) => {
                self.setup_requests.fetch_add(1, Ordering::Relaxed);
                response
                    .header("x-ms-sku-name")
                    .filter(|_| response.status.is_success())
                    .map(str::to_owned)
            }
            Err(_) => None,
        };
        let unknown = if sku.is_some() {
            "region"
        } else {
            "region and redundancy"
        };
        let placement = AccountPlacement {
            region: String::new(),
            sku: sku.unwrap_or_default(),
        };
        self.warnings.push(format!(
            "{account}: {unknown} unknown with {} (only found with `az login`), so priced at \
             {} rates",
            self.auth.describe(),
            placement.label()
        ));
        placement
    }

    /// Every storage account in every subscription the user can read (needs the Reader
    /// role, or any role with Microsoft.Storage/storageAccounts/read). A subscription
    /// that can't be read is warned about and left out; if none can, that's an error.
    async fn resource_manager_accounts(&mut self) -> Result<Vec<Account>> {
        let subscriptions: Vec<Subscription> = self
            .arm_list(format!("{ARM}/subscriptions?api-version=2022-12-01"))
            .await?;
        let this = &*self;
        let lookups: Vec<(String, Result<Vec<StorageAccount>>)> = stream::iter(subscriptions)
            .map(|subscription| async move {
                let url = format!(
                    "{ARM}/subscriptions/{}/providers/Microsoft.Storage/storageAccounts?api-version=2023-05-01",
                    encode(&subscription.subscription_id)
                );
                (subscription.subscription_id, this.arm_list(url).await)
            })
            .buffered(LOOKUPS_AT_ONCE)
            .collect()
            .await;

        let mut accounts = Vec::new();
        let mut warnings = Vec::new();
        let mut first_error = None;
        for (subscription, found) in lookups {
            match found {
                Ok(found) => accounts.extend(
                    found
                        .into_iter()
                        .filter(|account| location::is_account_name(&account.name))
                        .map(|account| Account {
                            name: account.name,
                            placement: AccountPlacement {
                                region: account.location,
                                sku: account.sku.map(|sku| sku.name).unwrap_or_default(),
                            },
                        }),
                ),
                Err(error) => {
                    warnings.push(format!(
                        "subscription {subscription}: storage accounts not listed. {error}"
                    ));
                    first_error.get_or_insert(error);
                }
            }
        }
        if accounts.is_empty()
            && let Some(error) = first_error
        {
            return Err(error);
        }
        self.warnings.extend(warnings);
        Ok(accounts)
    }

    async fn arm_list<T: for<'de> Deserialize<'de>>(&self, url: String) -> Result<Vec<T>> {
        let mut items = Vec::new();
        let mut next = Some(url);
        while let Some(url) = next {
            let token = self.tokens.token(MANAGEMENT).await?;
            let authorization = format!("Bearer {token}");
            let response = self
                .http
                .get(&url, &[("authorization", &authorization)])
                .await?;
            self.setup_requests.fetch_add(1, Ordering::Relaxed);
            if !response.status.is_success() {
                return Err(Error::Request {
                    operation: "List storage accounts",
                    permission: "the Reader role",
                    message: arm_error(&response),
                });
            }
            let page: ArmList<T> = serde_json::from_slice(&response.body).map_err(|error| {
                Error::Network(format!(
                    "unexpected Azure Resource Manager response: {error}"
                ))
            })?;
            items.extend(page.value);
            next = page.next_link.map(arm_link).transpose()?;
        }
        Ok(items)
    }

    async fn containers(&self, account: &str) -> Result<Vec<String>> {
        let mut containers = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut url =
                format!("https://{account}.blob.core.windows.net/?comp=list&maxresults=5000");
            if let Some(marker) = &marker {
                url.push_str(&format!("&marker={}", encode(marker)));
            }
            let response = self.auth.get(&self.http, &url, API_VERSION).await?;
            self.setup_requests.fetch_add(1, Ordering::Relaxed);
            check(&response, "List Containers", self.auth.needs(true), account)?;
            let page = parse_containers(&response.text()).map_err(bad_xml)?;
            containers.extend(page.0);
            match page.1 {
                Some(next) => marker = Some(next),
                None => return Ok(containers),
            }
        }
    }
}

/// A next-page link from Resource Manager, which must stay on Resource Manager: the
/// request that follows it carries the management token.
fn arm_link(link: String) -> Result<String> {
    if link.starts_with(&format!("{ARM}/")) {
        Ok(link)
    } else {
        Err(Error::Network(format!(
            "Azure Resource Manager sent a next page link to another host ({})",
            crate::http::host(&link)
        )))
    }
}

fn record_result(
    (name, result): (String, Result<()>),
    skipped: &mut Vec<SkippedBucket>,
) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(error) => {
            skipped.push(SkippedBucket {
                bucket: name,
                reason: error.to_string(),
            });
            Ok(())
        }
    }
}

/// Lists everything under `prefix`: expands prefixes breadth-first until there are
/// enough to keep `concurrency` requests busy, then lists each one fully in parallel.
async fn scan_container(lister: Lister, prefix: String, concurrency: usize) -> Result<()> {
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

#[derive(Clone)]
struct Lister {
    http: Http,
    auth: Arc<BlobAuth>,
    account: String,
    container: String,
    key_prefix: String,
    include_versions: bool,
    /// Accounts with a hierarchical namespace (Data Lake) cannot list versions; once
    /// that is known, the rest of the container is listed without them.
    versions_unsupported: Arc<AtomicBool>,
    list_price_per_1000: f64,
    requests: Arc<AtomicU64>,
    cost_nanodollars: Arc<AtomicU64>,
    slots: Arc<Semaphore>,
    warnings: Arc<Mutex<Vec<String>>>,
    sink: EntrySender,
}

impl Lister {
    /// Lists blobs under `prefix`, sending them to the tree, and returns the
    /// subprefixes when `split` asks for them (listing with a `/` delimiter).
    async fn list(&self, prefix: String, split: bool) -> Result<Vec<String>> {
        let _slot = self.slots.acquire().await.map_err(|_| Error::Cancelled)?;
        let mut subprefixes = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let versions =
                self.include_versions && !self.versions_unsupported.load(Ordering::Relaxed);
            let mut url = format!(
                "https://{}.blob.core.windows.net/{}?restype=container&comp=list&maxresults=5000&prefix={}",
                self.account,
                container_path(&self.container),
                encode(&prefix)
            );
            if split {
                url.push_str("&delimiter=%2F");
            }
            if versions {
                url.push_str("&include=versions%2Csnapshots%2Cdeleted");
            }
            if let Some(marker) = &marker {
                url.push_str(&format!("&marker={}", encode(marker)));
            }

            let response = self.auth.get(&self.http, &url, API_VERSION).await?;
            self.count_request();
            if versions && response.status.as_u16() == 400 {
                self.versions_unsupported.store(true, Ordering::Relaxed);
                self.warn(format!(
                    "{}/{}: versions and soft-deleted blobs not listed ({})",
                    self.account,
                    self.container,
                    azure_error(&response)
                ));
                continue;
            }
            check(
                &response,
                "List Blobs",
                self.auth.needs(false),
                &self.container,
            )?;

            let page = parse_blobs(&response.text()).map_err(bad_xml)?;
            let entries = page
                .blobs
                .into_iter()
                .map(|blob| self.entry(blob))
                .collect();
            self.sink
                .send(entries)
                .await
                .map_err(|_| Error::Cancelled)?;
            subprefixes.extend(page.prefixes);
            match page.next_marker {
                Some(next) => marker = Some(next),
                None => return Ok(subprefixes),
            }
        }
    }

    fn entry(&self, blob: Blob) -> Entry {
        let noncurrent = blob.deleted || blob.snapshot || (blob.version && !blob.current_version);
        Entry {
            key: format!("{}{}", self.key_prefix, blob.name),
            size: blob.size,
            storage_class: blob.tier.unwrap_or(Cow::Borrowed(DEFAULT_TIER)),
            kind: if noncurrent {
                EntryKind::Noncurrent
            } else {
                EntryKind::Current
            },
            last_modified: blob.last_modified,
        }
    }

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

/// Turns an error response from the Blob service into an [`Error`]. `permission` is
/// what the listing needs; `what` is the container (or account) being listed.
fn check(
    response: &Response,
    operation: &'static str,
    permission: &'static str,
    what: &str,
) -> Result<()> {
    let error = azure_error(response);
    match response.status.as_u16() {
        200..=299 => Ok(()),
        // A wrong account key or SAS signature comes back as 403 AuthenticationFailed.
        403 if error.starts_with("AuthenticationFailed") => Err(Error::Credentials(format!(
            "Azure rejected the credentials ({error}). Check the account key or SAS token."
        ))),
        // Only Azure CLI tokens get a 401. A token Azure understands but won't take here
        // is usually from another tenant than the storage account's.
        401 if error.starts_with("InvalidAuthenticationInfo") => {
            let tenant = response
                .header("www-authenticate")
                .and_then(tenant_of_challenge)
                .unwrap_or("TENANT_ID");
            Err(Error::Credentials(format!(
                "Azure rejected the Azure CLI's sign-in ({error}). The storage account is \
                 probably in a different Azure tenant than the one `az login` signed in to: run \
                 `az login --tenant {tenant}`, or use a SAS token or account key."
            )))
        }
        401 => Err(Error::Credentials(format!(
            "Azure rejected the credentials ({error}). Run `az login` to sign in again, or check \
             the SAS token or account key.",
        ))),
        404 => Err(Error::NoSuchBucket(what.to_owned())),
        _ => Err(Error::Request {
            operation,
            permission,
            message: error,
        }),
    }
}

/// The tenant a 401's `WWW-Authenticate` challenge asks for:
/// `Bearer authorization_uri=https://login.microsoftonline.com/TENANT/oauth2/authorize ...`.
fn tenant_of_challenge(challenge: &str) -> Option<&str> {
    let uri = challenge
        .split([' ', ','])
        .find_map(|part| part.strip_prefix("authorization_uri="))?
        .trim_matches('"');
    let path = uri.split_once("://")?.1.split_once('/')?.1;
    let tenant = path.split('/').next()?;
    let valid = !tenant.is_empty()
        && tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'));
    valid.then_some(tenant)
}

/// A container name as a URL path segment. `$` (in `$web`, `$logs`, `$root`) is legal in
/// a path, so it stays as is: Shared Key signs the path as sent, and a `%24` there
/// would have to be signed exactly the way Azure decodes it.
fn container_path(container: &str) -> String {
    encode(container).replace("%24", "$")
}

fn bad_xml(error: String) -> Error {
    Error::Network(format!("unexpected Blob Storage response: {error}"))
}

/// `Code: Message` of a Blob service error body, or the HTTP status.
fn azure_error(response: &Response) -> String {
    let mut code = String::new();
    let mut message = String::new();
    let _ = walk(&response.text(), |event| {
        if let XmlEvent::End(path, text, _) = event {
            match path {
                [error, name] if error == "Error" && name == "Code" => code = text.to_owned(),
                [error, name] if error == "Error" && name == "Message" => message = text.to_owned(),
                _ => {}
            }
        }
    });
    // The message ends with a request ID and time on their own lines.
    let message = message.lines().next().unwrap_or_default().trim();
    match (code.is_empty(), message.is_empty()) {
        (false, false) => format!("{code}: {message}"),
        (false, true) => code,
        _ => format!("HTTP {}", response.status),
    }
}

/// The message of an Azure Resource Manager error (`{"error": {"message": ...}}`).
fn arm_error(response: &Response) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: Detail,
    }
    #[derive(Deserialize)]
    struct Detail {
        code: String,
        message: String,
    }
    match serde_json::from_slice::<Body>(&response.body) {
        Ok(body) => format!("{}: {}", body.error.code, body.error.message),
        Err(_) => format!("HTTP {}", response.status),
    }
}

#[derive(Debug, Default, PartialEq)]
struct Blob {
    name: String,
    deleted: bool,
    snapshot: bool,
    version: bool,
    current_version: bool,
    size: u64,
    tier: Option<Cow<'static, str>>,
    /// Seconds since the Unix epoch.
    last_modified: Option<u64>,
}

#[derive(Debug, Default)]
struct BlobPage {
    blobs: Vec<Blob>,
    prefixes: Vec<String>,
    next_marker: Option<String>,
}

/// Blob names that XML cannot hold come percent-encoded, marked `Encoded="true"`.
fn blob_name(text: &str, encoded: bool) -> String {
    if encoded {
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    } else {
        text.to_owned()
    }
}

/// A marker for the next page; empty means there is none.
fn next_marker(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

fn parse_blobs(xml: &str) -> std::result::Result<BlobPage, String> {
    let mut page = BlobPage::default();
    let mut blob: Option<Blob> = None;
    walk(xml, |event| match event {
        XmlEvent::Start([.., list, item]) if list == "Blobs" && item == "Blob" => {
            blob = Some(Blob::default());
        }
        XmlEvent::Start(_) => {}
        XmlEvent::End(path, text, encoded) => match path {
            [.., list, item] if list == "Blobs" && item == "Blob" => {
                page.blobs.extend(blob.take());
            }
            [.., item, name] if item == "BlobPrefix" && name == "Name" => {
                page.prefixes.push(blob_name(text, encoded));
            }
            [root, marker] if root == "EnumerationResults" && marker == "NextMarker" => {
                page.next_marker = next_marker(text);
            }
            [.., item, field] if item == "Blob" => {
                if let Some(blob) = &mut blob {
                    match field.as_str() {
                        "Name" => blob.name = blob_name(text, encoded),
                        "Deleted" => blob.deleted = text == "true",
                        "Snapshot" => blob.snapshot = !text.is_empty(),
                        "VersionId" => blob.version = !text.is_empty(),
                        "IsCurrentVersion" => blob.current_version = text == "true",
                        _ => {}
                    }
                }
            }
            [.., item, properties, field] if item == "Blob" && properties == "Properties" => {
                if let Some(blob) = &mut blob {
                    match field.as_str() {
                        "Content-Length" => blob.size = text.parse().unwrap_or(0),
                        "AccessTier" => {
                            blob.tier = (!text.is_empty()).then(|| class_name(text, TIERS));
                        }
                        "Last-Modified" => blob.last_modified = http_date_seconds(text),
                        _ => {}
                    }
                }
            }
            _ => {}
        },
    })?;
    Ok(page)
}

/// Container names and the marker of the next page.
fn parse_containers(xml: &str) -> std::result::Result<(Vec<String>, Option<String>), String> {
    let mut containers = Vec::new();
    let mut next = None;
    walk(xml, |event| {
        if let XmlEvent::End(path, text, _) = event {
            match path {
                [.., item, name] if item == "Container" && name == "Name" => {
                    containers.push(text.to_owned());
                }
                [root, marker] if root == "EnumerationResults" && marker == "NextMarker" => {
                    next = next_marker(text);
                }
                _ => {}
            }
        }
    })?;
    Ok((containers, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<EnumerationResults ServiceEndpoint="https://acct.blob.core.windows.net/" ContainerName="photos">
  <Prefix/><MaxResults>5000</MaxResults><Delimiter>/</Delimiter>
  <Blobs>
    <Blob>
      <Name>a &amp; b.jpg</Name>
      <VersionId>2026-01-01T00:00:00.0000000Z</VersionId>
      <IsCurrentVersion>true</IsCurrentVersion>
      <Properties>
        <Last-Modified>Tue, 29 Sep 2026 00:00:00 GMT</Last-Modified>
        <Content-Length>1024</Content-Length>
        <BlobType>BlockBlob</BlobType>
        <AccessTier>Cool</AccessTier>
      </Properties>
      <Metadata/>
    </Blob>
    <Blob>
      <Name>a &amp; b.jpg</Name>
      <VersionId>2025-01-01T00:00:00.0000000Z</VersionId>
      <Properties><Content-Length>512</Content-Length></Properties>
    </Blob>
    <Blob>
      <Name Encoded="true">odd%01name</Name>
      <Deleted>true</Deleted>
      <Properties><Content-Length>7</Content-Length><AccessTier>Archive</AccessTier></Properties>
    </Blob>
    <BlobPrefix><Name>2024/</Name></BlobPrefix>
  </Blobs>
  <NextMarker>2!abc</NextMarker>
</EnumerationResults>"#;

    #[test]
    fn parses_blob_pages() {
        let page = parse_blobs(PAGE).unwrap();
        assert_eq!(page.prefixes, ["2024/"]);
        assert_eq!(page.next_marker.as_deref(), Some("2!abc"));
        assert_eq!(page.blobs.len(), 3);
        assert_eq!(
            page.blobs[0],
            Blob {
                name: "a & b.jpg".to_owned(),
                deleted: false,
                snapshot: false,
                version: true,
                current_version: true,
                size: 1024,
                tier: Some("Cool".into()),
                last_modified: Some(1_790_640_000),
            }
        );
        assert!(page.blobs[1].version && !page.blobs[1].current_version);
        assert_eq!(page.blobs[2].name, "odd\u{1}name");
        assert!(page.blobs[2].deleted);
    }

    #[test]
    fn finds_the_tenant_in_a_bearer_challenge() {
        let challenge = "Bearer authorization_uri=https://login.microsoftonline.com/\
                         00000000-1111-2222-3333-444444444444/oauth2/authorize \
                         resource_id=https://storage.azure.com";
        assert_eq!(
            tenant_of_challenge(challenge),
            Some("00000000-1111-2222-3333-444444444444")
        );
        assert_eq!(tenant_of_challenge("Bearer realm=\"x\""), None);
    }

    #[test]
    fn next_page_links_stay_on_resource_manager() {
        let link = format!("{ARM}/subscriptions?api-version=2022-12-01&$skiptoken=x");
        assert_eq!(arm_link(link.clone()).unwrap(), link);
        for other in [
            "https://evil.example/subscriptions",
            "https://management.azure.com.evil.example/x",
            "http://management.azure.com/x",
        ] {
            assert!(arm_link(other.to_owned()).is_err(), "{other}");
        }
    }

    #[test]
    fn special_containers_keep_their_dollar_sign() {
        assert_eq!(container_path("$web"), "$web");
        assert_eq!(container_path("photos-2024"), "photos-2024");
    }

    #[test]
    fn parses_container_pages() {
        let xml = r#"<EnumerationResults><Containers><Container><Name>logs</Name></Container><Container><Name>photos</Name></Container></Containers><NextMarker/></EnumerationResults>"#;
        let (containers, next) = parse_containers(xml).unwrap();
        assert_eq!(containers, ["logs", "photos"]);
        assert_eq!(next, None);
    }
}
