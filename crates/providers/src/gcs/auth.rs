//! Google sign-in: Application Default Credentials (what `gcloud auth
//! application-default login` saves, or a service account key file), or an access token
//! given directly. Tokens last an hour, so long scans refresh them as they go.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::SignatureScheme;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;
use rustls::sign::SigningKey;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::http::{Http, encode};
use crate::{Error, Result};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Where a service account key file may send its signed sign-in request: Google's token
/// endpoints, current and old.
const TOKEN_HOSTS: [&str; 2] = ["oauth2.googleapis.com", "accounts.google.com"];
/// Read-only access to Cloud Storage is all a scan needs.
const SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_only";
/// Refresh this long before a token expires, so no request carries a stale one.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);
const ADC_FILE: &str = "application_default_credentials.json";

/// How to sign in to Google Cloud.
#[derive(Clone, Default)]
pub struct GcsCredentials {
    /// An OAuth access token (e.g. from `gcloud auth print-access-token`) to use instead
    /// of Application Default Credentials. Valid for about an hour.
    pub access_token: Option<String>,
    /// The project whose buckets `gs://` lists. Defaults to the one in the credentials
    /// file or gcloud's configuration.
    pub project: Option<String>,
}

impl GcsCredentials {
    /// An access token from GOOGLE_OAUTH_ACCESS_TOKEN (the variable gcloud and the
    /// Google client libraries use), if set. With none, Application Default Credentials
    /// sign in. The project comes from elsewhere (see [`GcsCredentials::project`]).
    pub fn from_env() -> Self {
        Self {
            access_token: std::env::var("GOOGLE_OAUTH_ACCESS_TOKEN")
                .ok()
                .filter(|token| !token.trim().is_empty()),
            project: None,
        }
    }
}

impl fmt::Debug for GcsCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GcsCredentials")
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "** redacted **"),
            )
            .field("project", &self.project)
            .finish()
    }
}

/// Hands out a valid access token, refreshing it when it is about to expire.
pub(super) struct TokenSource {
    kind: Kind,
    http: Http,
    cached: Mutex<Option<(String, Instant)>>,
    /// The project named in the credentials file, if any.
    pub project: Option<String>,
}

enum Kind {
    Fixed(String),
    User {
        client_id: String,
        client_secret: String,
        refresh_token: String,
    },
    ServiceAccount {
        email: String,
        key: Arc<dyn SigningKey>,
        token_uri: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CredentialsFile {
    AuthorizedUser {
        client_id: String,
        client_secret: String,
        refresh_token: String,
        quota_project_id: Option<String>,
    },
    ServiceAccount {
        client_email: String,
        private_key: String,
        token_uri: Option<String>,
        project_id: Option<String>,
    },
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

impl TokenSource {
    pub fn new(credentials: &GcsCredentials, http: Http) -> Result<Self> {
        if let Some(token) = &credentials.access_token {
            return Ok(Self {
                kind: Kind::Fixed(token.trim().to_owned()),
                http,
                cached: Mutex::new(None),
                project: None,
            });
        }

        let path = adc_path().ok_or_else(no_credentials)?;
        let text = std::fs::read_to_string(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => no_credentials(),
            _ => Error::Credentials(format!("could not read {}: {error}", path.display())),
        })?;
        let file: CredentialsFile = serde_json::from_str(&text).map_err(|error| {
            Error::Credentials(format!(
                "{} is not a supported Google credentials file (user or service account): {error}",
                path.display()
            ))
        })?;
        let (kind, project) = match file {
            CredentialsFile::AuthorizedUser {
                client_id,
                client_secret,
                refresh_token,
                quota_project_id,
            } => (
                Kind::User {
                    client_id,
                    client_secret,
                    refresh_token,
                },
                quota_project_id,
            ),
            CredentialsFile::ServiceAccount {
                client_email,
                private_key,
                token_uri,
                project_id,
            } => (
                Kind::ServiceAccount {
                    email: client_email,
                    key: signing_key(&private_key)?,
                    token_uri: checked_token_uri(token_uri)?,
                },
                project_id,
            ),
        };
        Ok(Self {
            kind,
            http,
            cached: Mutex::new(None),
            project,
        })
    }

    pub async fn token(&self) -> Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some((token, expires)) = cached.as_ref()
            && Instant::now() + REFRESH_MARGIN < *expires
        {
            return Ok(token.clone());
        }
        let (token, lifetime) = match &self.kind {
            // A pasted token's expiry is unknown; if it has expired, requests say so.
            Kind::Fixed(token) => return Ok(token.clone()),
            Kind::User {
                client_id,
                client_secret,
                refresh_token,
            } => {
                let body = format!(
                    "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={}",
                    encode(client_id),
                    encode(client_secret),
                    encode(refresh_token)
                );
                self.exchange(TOKEN_URL, body).await?
            }
            Kind::ServiceAccount {
                email,
                key,
                token_uri,
            } => {
                let assertion = jwt(email, token_uri, key.as_ref())?;
                let body = format!(
                    "grant_type={}&assertion={assertion}",
                    encode("urn:ietf:params:oauth:grant-type:jwt-bearer")
                );
                self.exchange(token_uri, body).await?
            }
        };
        *cached = Some((token.clone(), Instant::now() + lifetime));
        Ok(token)
    }

    async fn exchange(&self, url: &str, body: String) -> Result<(String, Duration)> {
        let response = self.http.post_form(url, body).await?;
        if !response.status.is_success() {
            return Err(Error::Credentials(format!(
                "Google sign-in failed ({}): {}. Run `gcloud auth application-default login` \
                 to sign in again.",
                response.status,
                response.text().trim()
            )));
        }
        let token: TokenResponse = serde_json::from_slice(&response.body).map_err(|error| {
            Error::Credentials(format!("unexpected Google sign-in response: {error}"))
        })?;
        let lifetime = Duration::from_secs(token.expires_in.unwrap_or(3600));
        Ok((token.access_token, lifetime))
    }
}

fn no_credentials() -> Error {
    Error::Credentials(
        "no Google credentials found. Run `gcloud auth application-default login`, set \
         GOOGLE_APPLICATION_CREDENTIALS to a service account key file, or use an access \
         token (choose Access token, or set GOOGLE_OAUTH_ACCESS_TOKEN)"
            .to_owned(),
    )
}

/// Where Application Default Credentials live: `GOOGLE_APPLICATION_CREDENTIALS`, or the
/// file `gcloud auth application-default login` writes into gcloud's config folder.
fn adc_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        return Some(PathBuf::from(path));
    }
    gcloud_config_dir().map(|dir| dir.join(ADC_FILE))
}

/// gcloud's configuration folder: `CLOUDSDK_CONFIG`, `%APPDATA%\gcloud` on Windows,
/// or `~/.config/gcloud` elsewhere.
pub(super) fn gcloud_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLOUDSDK_CONFIG") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("gcloud"))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/gcloud"))
    }
}

/// The project gcloud is set to use (`gcloud config set project ...`), if any.
pub(super) fn gcloud_project() -> Option<String> {
    let dir = gcloud_config_dir()?;
    let active = std::fs::read_to_string(dir.join("active_config"))
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|_| "default".to_owned());
    let config =
        std::fs::read_to_string(dir.join("configurations").join(format!("config_{active}")))
            .ok()?;
    ini_value(&config, "core", "project")
}

/// A `key = value` from `[section]` of an INI file.
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            in_section = name.trim() == section;
        } else if in_section
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        }
    }
    None
}

/// The key file's `token_uri`, which must be one of Google's: the signed request sent
/// there is enough to sign in as the service account.
fn checked_token_uri(token_uri: Option<String>) -> Result<String> {
    let Some(uri) = token_uri else {
        return Ok(TOKEN_URL.to_owned());
    };
    let host = uri
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next());
    if host.is_some_and(|host| TOKEN_HOSTS.contains(&host)) {
        Ok(uri)
    } else {
        Err(Error::Credentials(format!(
            "the service account key file's token_uri is not a Google sign-in address: {}",
            host.unwrap_or("not https")
        )))
    }
}

fn signing_key(pem: &str) -> Result<Arc<dyn SigningKey>> {
    let invalid = |error: String| {
        Error::Credentials(format!(
            "the service account's private key is invalid: {error}"
        ))
    };
    let key = PrivateKeyDer::from_pem_slice(pem.as_bytes())
        .map_err(|error| invalid(error.to_string()))?;
    rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)
        .map_err(|error| invalid(error.to_string()))
}

/// A signed JSON Web Token asking for a Cloud Storage read-only token for a service
/// account (RFC 7523).
fn jwt(email: &str, audience: &str, key: &dyn SigningKey) -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let header = base64url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iss": email,
        "scope": SCOPE,
        "aud": audience,
        "iat": now,
        "exp": now + 3600,
    });
    let payload = format!("{header}.{}", base64url(claims.to_string().as_bytes()));
    let signer = key
        .choose_scheme(&[SignatureScheme::RSA_PKCS1_SHA256])
        .ok_or_else(|| {
            Error::Credentials("the service account key is not an RSA key".to_owned())
        })?;
    let signature = signer
        .sign(payload.as_bytes())
        .map_err(|error| Error::Credentials(format!("could not sign in as {email}: {error}")))?;
    Ok(format!("{payload}.{}", base64url(&signature)))
}

/// Base64 with the URL-safe alphabet and no padding, as JWTs use.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (index, &byte)| {
            n | u32::from(byte) << (16 - 8 * index)
        });
        for index in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(n >> (18 - 6 * index) & 63) as usize]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_without_padding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn reads_the_project_from_gcloud_config() {
        let config = "[core]\naccount = me@example.com\nproject = my-project\n\n[compute]\nregion = us-east1\n";
        assert_eq!(
            ini_value(config, "core", "project").as_deref(),
            Some("my-project")
        );
        assert_eq!(ini_value(config, "compute", "project"), None);
    }

    #[test]
    fn debug_output_hides_the_token() {
        let credentials = GcsCredentials {
            access_token: Some("ya29.secret".to_owned()),
            project: Some("p".to_owned()),
        };
        assert!(!format!("{credentials:?}").contains("ya29"));
    }

    #[test]
    fn service_accounts_sign_in_only_at_google() {
        assert_eq!(checked_token_uri(None).unwrap(), TOKEN_URL);
        for good in [
            "https://oauth2.googleapis.com/token",
            "https://accounts.google.com/o/oauth2/token",
        ] {
            assert_eq!(checked_token_uri(Some(good.to_owned())).unwrap(), good);
        }
        for bad in [
            "https://evil.example/token",
            "https://oauth2.googleapis.com.evil.example/token",
            "https://evil.example?oauth2.googleapis.com/token",
            "http://oauth2.googleapis.com/token",
        ] {
            assert!(checked_token_uri(Some(bad.to_owned())).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_both_kinds_of_credentials_file() {
        let user = r#"{"type":"authorized_user","client_id":"id","client_secret":"s","refresh_token":"r","quota_project_id":"p"}"#;
        assert!(matches!(
            serde_json::from_str::<CredentialsFile>(user),
            Ok(CredentialsFile::AuthorizedUser { .. })
        ));
        let other = r#"{"type":"external_account"}"#;
        assert!(serde_json::from_str::<CredentialsFile>(other).is_err());
    }
}
