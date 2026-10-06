//! Azure Blob Storage, through the Blob REST API; storage accounts and their regions
//! come from Azure Resource Manager.

mod arm;
mod auth;
mod location;
mod parse;
mod prices;
mod pricing;

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use clouddirstat_core::{Entry, EntryKind};
use futures_util::{StreamExt, stream};

pub use auth::AzureCredentials;
pub use location::AzureLocation;
pub use pricing::{AccountPlacement, AzurePricing};

use self::auth::{BlobAuth, CliTokens, STORAGE};
use self::parse::{Blob, azure_error, bad_xml, parse_blobs, parse_containers};
use crate::http::{Http, Response, encode};
use crate::listing::{self, PrefixLister, ScanContext};
use crate::{EntrySender, Error, Result, ScanOptions, ScanStats, SkippedBucket};

const API_VERSION: &str = "2023-11-03";
const DEFAULT_TIER: &str = "Hot";
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
        let context = ScanContext::new(
            options.concurrency,
            sink,
            self.setup_requests.load(Ordering::Relaxed),
            0.0,
            self.warnings.clone(),
        );
        let lister = |target: &Target| Lister {
            http: self.http.clone(),
            auth: Arc::clone(&self.auth),
            account: target.account.clone(),
            container: target.container.clone(),
            key_prefix: target.key_prefix.clone(),
            include_versions: options.include_versions,
            versions_unsupported: Arc::new(AtomicBool::new(false)),
            list_price_per_1000: target.list_price_per_1000,
            context: context.clone(),
        };

        let mut skipped = self.skipped.clone();
        if self.targets.len() == 1 && !self.location.container.is_empty() {
            let prefix = self.location.prefix.clone();
            listing::list_prefix(lister(&self.targets[0]), prefix, options.concurrency).await?;
        } else {
            // Collected before awaiting: holding the lazy iterator across `.await`
            // would keep this future from being `Send`.
            let scans: Vec<_> = self
                .targets
                .iter()
                .map(|target| {
                    let scan =
                        listing::list_prefix(lister(target), String::new(), options.concurrency);
                    (target.key_prefix.trim_end_matches('/').to_owned(), scan)
                })
                .collect();
            listing::each_bucket(scans, &mut skipped).await?;
        }
        let default_price = AzurePricing::list_price_per_1000(&AccountPlacement::default());
        Ok(context.stats(skipped, default_price))
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
            containers.extend(page.names);
            match page.next_marker {
                Some(next) => marker = Some(next),
                None => return Ok(containers),
            }
        }
    }
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
    context: ScanContext,
}

impl PrefixLister for Lister {
    async fn list(&self, prefix: String, split: bool) -> Result<Vec<String>> {
        let _slot = self.context.slot().await?;
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
            self.context.count_request(self.list_price_per_1000);
            // Data Lake accounts reject listing versions outright, on the first page; a
            // 400 later on has some other cause, and is reported as the error it is.
            if versions && marker.is_none() && response.status.as_u16() == 400 {
                self.versions_unsupported.store(true, Ordering::Relaxed);
                self.context.warn(format!(
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
            self.context.send(entries).await?;
            subprefixes.extend(page.prefixes);
            match page.next_marker {
                Some(next) => marker = Some(next),
                None => return Ok(subprefixes),
            }
        }
    }
}

impl Lister {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn special_containers_keep_their_dollar_sign() {
        assert_eq!(container_path("$web"), "$web");
        assert_eq!(container_path("photos-2024"), "photos-2024");
    }
}
