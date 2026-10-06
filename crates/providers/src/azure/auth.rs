//! Azure sign-in: access tokens from the Azure CLI (`az login`), a SAS token, or a
//! storage account key.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aws_lc_rs::hmac;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::location::is_account_name;
use crate::http::{Http, Response};
use crate::time::http_date;
use crate::{Error, Result};

/// Blob data (listing containers and blobs).
pub(super) const STORAGE: &str = "https://storage.azure.com/";
/// Azure Resource Manager (listing accounts, and their regions and redundancy).
pub(super) const MANAGEMENT: &str = "https://management.azure.com/";
/// Refresh this long before a token expires, so no request carries a stale one.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);
/// For CLI versions that do not say when a token expires.
const ASSUMED_LIFETIME: Duration = Duration::from_secs(45 * 60);

/// How to sign in to Azure.
#[derive(Clone, Default)]
pub struct AzureCredentials {
    /// A shared access signature (the `sv=...&sig=...` query string) to use instead of
    /// the Azure CLI. It covers one account (or container), so `az://` needs the CLI.
    pub sas: Option<String>,
    /// A storage account key, or a connection string holding one, to use instead of the
    /// Azure CLI. Like a SAS, it covers one account.
    pub account_key: Option<String>,
}

impl fmt::Debug for AzureCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |secret: &Option<String>| secret.as_ref().map(|_| "** redacted **");
        f.debug_struct("AzureCredentials")
            .field("sas", &redacted(&self.sas))
            .field("account_key", &redacted(&self.account_key))
            .finish()
    }
}

impl AzureCredentials {
    /// An account key from AZURE_STORAGE_KEY (or a connection string from
    /// AZURE_STORAGE_CONNECTION_STRING) and a SAS from AZURE_STORAGE_SAS_TOKEN, the
    /// variables the Azure CLI uses. With none set, the Azure CLI signs in.
    pub fn from_env() -> Self {
        let var = |name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        Self::sorted(
            var("AZURE_STORAGE_KEY").or_else(|| var("AZURE_STORAGE_CONNECTION_STRING")),
            var("AZURE_STORAGE_SAS_TOKEN"),
        )
    }

    /// Files a bare SAS given as a key or connection string under `sas`, so the GUI
    /// opens on the right sign-in mode.
    fn sorted(account_key: Option<String>, sas: Option<String>) -> Self {
        match (account_key, sas) {
            (Some(key), None) if is_bare_sas(&key) => Self {
                sas: Some(key),
                account_key: None,
            },
            (account_key, sas) => Self { sas, account_key },
        }
    }

    /// How to sign Blob service requests to `account`. Empty means every account, which
    /// only the Azure CLI can list; with a connection string it means the account the
    /// string is for.
    pub(super) fn blob_auth(&self, account: &str, tokens: &Arc<CliTokens>) -> Result<BlobAuth> {
        let secret = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let (auth, kind) = match (secret(&self.account_key), secret(&self.sas)) {
            // A connection string can hold a SAS instead of a key.
            (Some(key), _) if is_sas_connection_string(&key) => {
                (sas_from_connection_string(&key, account)?, "a SAS token")
            }
            // A bare SAS given where a key or connection string was expected.
            (Some(key), _) if is_bare_sas(&key) => (
                BlobAuth::Sas {
                    query: clean_sas(&key),
                    account: String::new(),
                },
                "a SAS token",
            ),
            (Some(key), _) => (
                BlobAuth::SharedKey(Box::new(SharedKey::new(&key, account)?)),
                "an account key",
            ),
            (None, Some(sas)) => (
                BlobAuth::Sas {
                    query: clean_sas(&sas),
                    account: String::new(),
                },
                "a SAS token",
            ),
            (None, None) => return Ok(BlobAuth::Cli(Arc::clone(tokens))),
        };
        if account.is_empty() && auth.account().is_none() {
            return Err(Error::Unsupported(format!(
                "az:// lists every account through the Azure CLI, but {kind} covers only one: \
                 enter az://ACCOUNT, or use a connection string, which names its account"
            )));
        }
        Ok(auth)
    }
}

/// How Blob service requests are signed.
pub(super) enum BlobAuth {
    Cli(Arc<CliTokens>),
    /// The SAS query string, without a leading `?`, and the account it is for when a
    /// connection string says (empty otherwise).
    Sas {
        query: String,
        account: String,
    },
    SharedKey(Box<SharedKey>),
}

impl BlobAuth {
    pub fn uses_cli(&self) -> bool {
        matches!(self, Self::Cli(_))
    }

    /// The one account these credentials are for, when they say (a connection string).
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::SharedKey(key) if !key.account.is_empty() => Some(&key.account),
            Self::Sas { account, .. } if !account.is_empty() => Some(account),
            _ => None,
        }
    }

    /// What the scan was signed in with, for messages about it.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Cli(_) => "the Azure CLI",
            Self::Sas { .. } => "a SAS token",
            Self::SharedKey(_) => "an account key",
        }
    }

    /// What a listing needs from these credentials, for "not authorized" errors.
    /// Listing containers (as opposed to blobs) needs more from a SAS.
    pub fn needs(&self, containers: bool) -> &'static str {
        match (self, containers) {
            (Self::Sas { .. }, true) => {
                "a SAS for the Blob service with the Service and Container resource types and List permission"
            }
            (Self::Sas { .. }, false) => {
                "a SAS for the Blob service with the Container resource type and List permission"
            }
            (Self::SharedKey(_), _) => "a valid account key",
            (Self::Cli(_), _) => "the Storage Blob Data Reader role",
        }
    }

    /// A signed GET on the Blob service. `url` must have a query string.
    pub async fn get(&self, http: &Http, url: &str, version: &str) -> Result<Response> {
        let version = ("x-ms-version", version);
        match self {
            Self::Cli(tokens) => {
                let authorization = format!("Bearer {}", tokens.token(STORAGE).await?);
                http.get(url, &[version, ("authorization", &authorization)])
                    .await
            }
            Self::Sas { query, .. } => http.get(&format!("{url}&{query}"), &[version]).await,
            Self::SharedKey(key) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| since.as_secs());
                let date = http_date(now);
                let authorization = key.authorization(url, &date, version.1);
                http.get(
                    url,
                    &[
                        version,
                        ("x-ms-date", &date),
                        ("authorization", &authorization),
                    ],
                )
                .await
            }
        }
    }
}

/// A storage account key, for Shared Key authorization.
pub(super) struct SharedKey {
    account: String,
    key: hmac::Key,
}

impl SharedKey {
    /// From a bare base64 key, or a connection string
    /// (`DefaultEndpointsProtocol=https;AccountName=...;AccountKey=...`), which must be
    /// for `account`. An empty `account` takes the connection string's.
    fn new(key_or_connection_string: &str, account: &str) -> Result<Self> {
        let mut key = key_or_connection_string;
        let mut account = account.to_owned();
        if key.contains("AccountKey=") {
            let field = |name| connection_field(key_or_connection_string, name);
            if let Some(named) = field("AccountName") {
                account = connection_account(named, &account)?;
            }
            check_endpoint_suffix(field("EndpointSuffix"))?;
            key = field("AccountKey").unwrap_or_default();
        }
        let bytes = BASE64.decode(key).map_err(|_| {
            Error::Credentials(
                "the account key is not valid: paste key1 or key2 from the storage account's \
                 Access keys page, or its connection string"
                    .to_owned(),
            )
        })?;
        Ok(Self {
            account,
            key: hmac::Key::new(hmac::HMAC_SHA256, &bytes),
        })
    }

    /// The `Authorization` header for a GET with no body and only the `x-ms-date` and
    /// `x-ms-version` headers.
    fn authorization(&self, url: &str, date: &str, version: &str) -> String {
        let string_to_sign = format!(
            "GET\n\n\n\n\n\n\n\n\n\n\n\nx-ms-date:{date}\nx-ms-version:{version}\n{}",
            canonical_resource(&self.account, url)
        );
        let signature = hmac::sign(&self.key, string_to_sign.as_bytes());
        format!(
            "SharedKey {}:{}",
            self.account,
            BASE64.encode(signature.as_ref())
        )
    }
}

/// A SAS as pasted: without a leading `?`, and without whitespace (from line wrapping),
/// which a SAS never contains.
fn clean_sas(sas: &str) -> String {
    let sas: String = sas.split_whitespace().collect();
    sas.trim_start_matches('?').to_owned()
}

/// `name=value` from a `;`-separated connection string (names in any case).
fn connection_field<'a>(connection_string: &'a str, name: &str) -> Option<&'a str> {
    connection_string
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

/// A SAS query string (`sv=...&sig=...`) rather than a key or connection string. A key
/// is base64, which has no `&`.
fn is_bare_sas(text: &str) -> bool {
    let text = text.trim_start_matches('?');
    text.contains('&') && text.split('&').any(|part| part.starts_with("sig="))
}

fn is_sas_connection_string(text: &str) -> bool {
    connection_field(text, "SharedAccessSignature").is_some()
        && connection_field(text, "AccountKey").is_none()
}

/// The account a connection string names, which must be `account` unless that is empty.
fn connection_account(named: &str, account: &str) -> Result<String> {
    let named = named.to_ascii_lowercase();
    if !is_account_name(&named) {
        return Err(Error::Credentials(format!(
            "the connection string names {named:?}, which is not a storage account name"
        )));
    }
    if !account.is_empty() && named != account {
        return Err(Error::Credentials(format!(
            "the connection string is for account {named}, not {account}"
        )));
    }
    Ok(named)
}

fn check_endpoint_suffix(suffix: Option<&str>) -> Result<()> {
    match suffix {
        Some(suffix) if !suffix.eq_ignore_ascii_case("core.windows.net") => {
            Err(Error::Credentials(format!(
                "only the public Azure cloud (core.windows.net) is supported, not {suffix}"
            )))
        }
        _ => Ok(()),
    }
}

/// A connection string with a SAS (`BlobEndpoint=https://ACCOUNT.blob.core.windows.net/;
/// SharedAccessSignature=sv=...`). It names its account in `AccountName` or in the
/// endpoint's host.
fn sas_from_connection_string(text: &str, account: &str) -> Result<BlobAuth> {
    let field = |name| connection_field(text, name);
    check_endpoint_suffix(field("EndpointSuffix"))?;
    let endpoint_account = match field("BlobEndpoint") {
        Some(endpoint) => {
            let host = endpoint
                .split_once("://")
                .map_or(endpoint, |(_, rest)| rest)
                .split('/')
                .next()
                .unwrap_or_default();
            let Some(name) = host.strip_suffix(".blob.core.windows.net") else {
                return Err(Error::Credentials(format!(
                    "only the public Azure cloud (blob.core.windows.net) is supported, not {host}"
                )));
            };
            Some(name)
        }
        None => None,
    };
    let account = match field("AccountName").or(endpoint_account) {
        Some(named) => connection_account(named, account)?,
        None => account.to_owned(),
    };
    Ok(BlobAuth::Sas {
        query: clean_sas(field("SharedAccessSignature").unwrap_or_default()),
        account,
    })
}

/// `/account/path` followed by each query parameter, decoded and sorted by name, as
/// Shared Key authorization signs it.
fn canonical_resource(account: &str, url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.find('/').map_or("/", |start| &path[start..]);
    let mut parameters: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |text: &str| {
            percent_encoding::percent_decode_str(text)
                .decode_utf8_lossy()
                .into_owned()
        };
        parameters
            .entry(decode(name).to_lowercase())
            .or_default()
            .push(decode(value));
    }
    let mut resource = format!("/{account}{path}");
    for (name, mut values) in parameters {
        values.sort();
        resource.push_str(&format!("\n{name}:{}", values.join(",")));
    }
    resource
}

/// Tokens from `az account get-access-token`, cached per resource until they are
/// about to expire.
#[derive(Default)]
pub(super) struct CliTokens {
    cached: Mutex<HashMap<&'static str, (String, Instant)>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliToken {
    access_token: String,
    /// Seconds since the Unix epoch (Azure CLI 2.54 and later).
    #[serde(rename = "expires_on")]
    expires_on: Option<u64>,
}

impl CliTokens {
    pub async fn token(&self, resource: &'static str) -> Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some((token, expires)) = cached.get(resource)
            && Instant::now() + REFRESH_MARGIN < *expires
        {
            return Ok(token.clone());
        }
        let token = tokio::task::spawn_blocking(move || cli_token(resource)).await??;
        let lifetime = token
            .expires_on
            .and_then(|expires| {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
                expires.checked_sub(now).map(Duration::from_secs)
            })
            .unwrap_or(ASSUMED_LIFETIME);
        cached.insert(
            resource,
            (token.access_token.clone(), Instant::now() + lifetime),
        );
        Ok(token.access_token)
    }
}

/// Where Homebrew installs `az`. An app opened from the macOS Finder (or from some Linux
/// desktops) searches only the system folders for programs, so `az` is looked for here
/// when it isn't on the search path.
const USUAL_AZ_PLACES: [&str; 3] = [
    "/opt/homebrew/bin/az",
    "/usr/local/bin/az",
    "/home/linuxbrew/.linuxbrew/bin/az",
];

/// The Azure CLI to run: `az` from the search path, else from Homebrew's folders.
fn az_program() -> PathBuf {
    // `az` is a .cmd script on Windows, which has to be named in full.
    if cfg!(windows) {
        return PathBuf::from("az.cmd");
    }
    find_az(
        std::env::var_os("PATH").as_deref(),
        &USUAL_AZ_PLACES,
        |path| path.is_file(),
    )
}

fn find_az(
    search_path: Option<&OsStr>,
    places: &[&str],
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let on_search_path = search_path
        .is_some_and(|dirs| std::env::split_paths(dirs).any(|dir| exists(&dir.join("az"))));
    if on_search_path {
        return PathBuf::from("az");
    }
    places
        .iter()
        .map(PathBuf::from)
        .find(|place| exists(place))
        .unwrap_or_else(|| PathBuf::from("az"))
}

fn cli_token(resource: &str) -> Result<CliToken> {
    let mut command = Command::new(az_program());
    command.args([
        "account",
        "get-access-token",
        "--resource",
        resource,
        "--output",
        "json",
    ]);
    hide_console_window(&mut command);
    let output = command.output().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Error::Credentials(
            "the Azure CLI (az) was not found: install it and run `az login`, or use a SAS \
             token"
                .to_owned(),
        ),
        _ => Error::Credentials(format!("could not run the Azure CLI: {error}")),
    })?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no details");
        return Err(Error::Credentials(format!(
            "could not get an Azure token (run `az login`): {message}"
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| Error::Credentials(format!("unexpected Azure CLI output: {error}")))
}

/// The GUI has no console, so without this every token refresh would flash one open.
fn hide_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_az_outside_the_search_path() {
        let installed = |places: &'static [&'static str]| {
            move |path: &Path| places.iter().any(|place| path == Path::new(place))
        };
        let places = ["/opt/homebrew/bin/az", "/usr/local/bin/az"];
        // Joined with the platform's separator (`:`, or `;` on Windows).
        let search_path = |dirs: &[&str]| std::env::join_paths(dirs).unwrap();
        // On the search path: run it by name.
        let path = search_path(&["/usr/bin", "/opt/homebrew/bin"]);
        assert_eq!(
            find_az(Some(&path), &places, installed(&["/opt/homebrew/bin/az"])),
            PathBuf::from("az")
        );
        // A Finder-launched app's search path lacks Homebrew's folder.
        let finder = search_path(&["/usr/bin", "/bin", "/usr/sbin", "/sbin"]);
        assert_eq!(
            find_az(Some(&finder), &places, installed(&["/usr/local/bin/az"])),
            PathBuf::from("/usr/local/bin/az")
        );
        // Not installed anywhere: `az` by name, so the error says it wasn't found.
        assert_eq!(find_az(None, &places, installed(&[])), PathBuf::from("az"));
    }

    #[test]
    fn parses_cli_tokens_with_and_without_expiry() {
        let new: CliToken = serde_json::from_str(
            r#"{"accessToken":"eyJ0","expiresOn":"2026-09-29 18:00:00.000000","expires_on":1790700000,"subscription":"s","tenant":"t","tokenType":"Bearer"}"#,
        )
        .unwrap();
        assert_eq!(new.expires_on, Some(1_790_700_000));
        let old: CliToken = serde_json::from_str(
            r#"{"accessToken":"eyJ0","expiresOn":"2026-09-29 18:00:00.000000"}"#,
        )
        .unwrap();
        assert_eq!(old.expires_on, None);
    }

    #[test]
    fn sas_loses_its_question_mark_and_stays_out_of_debug_output() {
        let credentials = AzureCredentials {
            sas: Some(" ?sv=2024&sig=secret ".to_owned()),
            account_key: None,
        };
        let tokens = Arc::new(CliTokens::default());
        match credentials.blob_auth("acct", &tokens).unwrap() {
            BlobAuth::Sas { query, .. } => assert_eq!(query, "sv=2024&sig=secret"),
            _ => panic!("expected a SAS"),
        }
        assert!(credentials.blob_auth("", &tokens).is_err());
        assert!(!format!("{credentials:?}").contains("secret"));
    }

    #[test]
    fn reads_account_keys_and_connection_strings() {
        let tokens = Arc::new(CliTokens::default());
        let with = |account_key: &str| AzureCredentials {
            sas: None,
            account_key: Some(account_key.to_owned()),
        };
        let connection = "DefaultEndpointsProtocol=https;AccountName=acct;AccountKey=a2V5;\
                          EndpointSuffix=core.windows.net";
        assert!(matches!(
            with(connection).blob_auth("acct", &tokens),
            Ok(BlobAuth::SharedKey(_))
        ));
        assert!(with(connection).blob_auth("other", &tokens).is_err());
        // az:// with a connection string scans the account it names; a bare key cannot.
        let auth = with(connection).blob_auth("", &tokens).unwrap();
        assert_eq!(auth.account(), Some("acct"));
        assert!(with("a2V5").blob_auth("", &tokens).is_err());
        assert!(matches!(
            with("a2V5").blob_auth("acct", &tokens),
            Ok(BlobAuth::SharedKey(_))
        ));
        assert!(with("not base64!").blob_auth("acct", &tokens).is_err());
        assert!(!format!("{:?}", with(connection)).contains("a2V5"));
    }

    #[test]
    fn sas_loses_whitespace_from_line_wrapping() {
        assert_eq!(
            clean_sas("?sv=2024&se=x\n  &sig=abc "),
            "sv=2024&se=x&sig=abc"
        );
    }

    #[test]
    fn a_bare_sas_in_the_key_field_is_used_as_a_sas() {
        let tokens = Arc::new(CliTokens::default());
        let credentials = AzureCredentials {
            sas: None,
            account_key: Some("?sv=2024&ss=b&srt=c&sp=rl&sig=abc%3D".to_owned()),
        };
        match credentials.blob_auth("acct", &tokens).unwrap() {
            BlobAuth::Sas { query, .. } => assert_eq!(query, "sv=2024&ss=b&srt=c&sp=rl&sig=abc%3D"),
            _ => panic!("expected a SAS"),
        }
        // Like any SAS it doesn't name its account, so az:// can't use it.
        assert!(credentials.blob_auth("", &tokens).is_err());
    }

    #[test]
    fn a_bare_sas_from_the_environment_is_filed_as_a_sas() {
        let sas = "sv=2024&sp=rl&sig=abc".to_owned();
        let credentials = AzureCredentials::sorted(Some(sas.clone()), None);
        assert_eq!(
            (credentials.sas, credentials.account_key),
            (Some(sas), None)
        );
        let key = AzureCredentials::sorted(Some("a2V5".to_owned()), None);
        assert_eq!(key.account_key.as_deref(), Some("a2V5"));
    }

    #[test]
    fn reads_sas_connection_strings() {
        let tokens = Arc::new(CliTokens::default());
        let credentials = AzureCredentials {
            sas: None,
            account_key: Some(
                "BlobEndpoint=https://acct.blob.core.windows.net/;SharedAccessSignature=sv=2024&sig=abc"
                    .to_owned(),
            ),
        };
        let auth = credentials.blob_auth("", &tokens).unwrap();
        assert_eq!(auth.account(), Some("acct"));
        match auth {
            BlobAuth::Sas { query, .. } => assert_eq!(query, "sv=2024&sig=abc"),
            _ => panic!("expected a SAS"),
        }
        assert!(credentials.blob_auth("other", &tokens).is_err());
        assert!(credentials.blob_auth("acct", &tokens).is_ok());
    }

    #[test]
    fn connection_strings_must_name_a_real_account() {
        let tokens = Arc::new(CliTokens::default());
        let with = |account_key: &str| AzureCredentials {
            sas: None,
            account_key: Some(account_key.to_owned()),
        };
        for connection in [
            "BlobEndpoint=https://evil.example?.blob.core.windows.net/;SharedAccessSignature=sv=1&sig=a",
            "BlobEndpoint=https://evil.example#.blob.core.windows.net/;SharedAccessSignature=sv=1&sig=a",
            "AccountName=evil.example?;AccountKey=a2V5",
            "AccountName=evil.example/x;AccountKey=a2V5",
        ] {
            assert!(
                matches!(
                    with(connection).blob_auth("", &tokens),
                    Err(Error::Credentials(_))
                ),
                "{connection}"
            );
        }
        let upper = with("AccountName=Acct;AccountKey=a2V5").blob_auth("acct", &tokens);
        assert_eq!(upper.unwrap().account(), Some("acct"));
    }

    #[test]
    fn canonicalizes_resources_for_shared_key() {
        assert_eq!(
            canonical_resource(
                "acct",
                "https://acct.blob.core.windows.net/photos?restype=container&comp=list&prefix=a%20b%2F&include=versions%2Csnapshots"
            ),
            "/acct/photos\ncomp:list\ninclude:versions,snapshots\nprefix:a b/\nrestype:container"
        );
        assert_eq!(
            canonical_resource("acct", "https://acct.blob.core.windows.net/?comp=list"),
            "/acct/\ncomp:list"
        );
    }

    /// The expected signature is HMAC-SHA256 of the documented string-to-sign, computed
    /// separately with Python's `hmac`.
    #[test]
    fn signs_with_shared_key() {
        let key = SharedKey::new("a2V5", "acct").unwrap();
        let authorization = key.authorization(
            "https://acct.blob.core.windows.net/?comp=list",
            "Tue, 29 Sep 2026 00:00:00 GMT",
            "2023-11-03",
        );
        assert_eq!(
            authorization,
            "SharedKey acct:S/Ziuu+Q7Z1a18SqtMsc4r90VBvUUhPBKB1wndX2gjQ="
        );
    }
}
