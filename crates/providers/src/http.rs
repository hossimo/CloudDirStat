//! A small HTTPS client for the providers that are called over plain REST (Google Cloud
//! Storage and Azure), built on the same hyper and rustls the AWS SDK already brings in.

use std::borrow::Cow;
use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Bytes;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

use crate::{Error, Result};

/// Gives up on a request that takes longer than this, e.g. a stalled connection.
const TIMEOUT: Duration = Duration::from_secs(60);
/// Attempts for throttled (429) and server (5xx) errors, and dropped connections.
const ATTEMPTS: u32 = 4;
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
/// The longest wait between attempts, whatever `Retry-After` asks for.
const MAX_WAIT: Duration = Duration::from_secs(30);
/// Far more than any listing page (Azure's 5,000 blobs are a few MB), so a response that
/// never ends can't use up memory.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Http {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

pub(crate) struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Response {
    /// The body as text, borrowed unless it isn't valid UTF-8.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// A header's value, when present and plain text.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }
}

impl Http {
    pub fn new() -> Result<Self> {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(rustls::crypto::aws_lc_rs::default_provider())
            .map_err(|error| Error::Network(format!("could not load root certificates: {error}")))?
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
        })
    }

    pub async fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response> {
        self.send(Method::GET, url, headers, Bytes::new()).await
    }

    pub async fn post_form(&self, url: &str, body: String) -> Result<Response> {
        let headers = [("content-type", "application/x-www-form-urlencoded")];
        self.send(Method::POST, url, &headers, Bytes::from(body))
            .await
    }

    /// Sends a request, retrying throttling, server errors, and dropped connections with
    /// exponential backoff (or as long as `Retry-After` asks). Other responses
    /// (including 4xx errors) are returned as is.
    async fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Bytes,
    ) -> Result<Response> {
        let mut backoff = FIRST_BACKOFF;
        let mut attempt = 1;
        loop {
            let mut request = Request::builder().method(method.clone()).uri(url);
            for &(name, value) in headers {
                request = request.header(name, value);
            }
            // Only the host: the query string may hold a SAS token.
            let request = request.body(Full::new(body.clone())).map_err(|error| {
                Error::Network(format!("invalid request to {}: {error}", host(url)))
            })?;

            let (result, retry, asked_wait) =
                match tokio::time::timeout(TIMEOUT, self.round_trip(request)).await {
                    Ok(Ok(response)) => {
                        let retry = response.status == StatusCode::TOO_MANY_REQUESTS
                            || response.status.is_server_error();
                        let asked_wait = retry_after(&response);
                        (Ok(response), retry, asked_wait)
                    }
                    Ok(Err(failure)) => (Err(failure.error), failure.transient, None),
                    Err(_) => {
                        let error = Error::Network(format!(
                            "no response from {} within {}s",
                            host(url),
                            TIMEOUT.as_secs()
                        ));
                        (Err(error), true, None)
                    }
                };
            if !retry || attempt == ATTEMPTS {
                return result;
            }
            let wait = asked_wait.unwrap_or_else(|| jittered(backoff));
            tokio::time::sleep(wait.min(MAX_WAIT)).await;
            backoff *= 2;
            attempt += 1;
        }
    }

    async fn round_trip(
        &self,
        request: Request<Full<Bytes>>,
    ) -> std::result::Result<Response, Failure> {
        let host = host(&request.uri().to_string()).to_owned();
        let response = self
            .client
            .request(request)
            .await
            .map_err(|error| Failure {
                // A certificate that doesn't check out won't on the next try either.
                transient: !is_tls_error(&error),
                error: Error::Network(format!("could not reach {host}: {error}")),
            })?;
        let (parts, body) = response.into_parts();
        let body = Limited::new(body, MAX_BODY_BYTES)
            .collect()
            .await
            .map_err(|error| Failure {
                transient: !error.is::<LengthLimitError>(),
                error: Error::Network(format!("response from {host} failed: {error}")),
            })?
            .to_bytes();
        Ok(Response {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }
}

/// A request that got no usable response, and whether trying again might help.
struct Failure {
    error: Error,
    transient: bool,
}

/// How long a throttled or unavailable service asks to wait, in whole seconds.
fn retry_after(response: &Response) -> Option<Duration> {
    let seconds = response.header("retry-after")?.trim().parse().ok()?;
    Some(Duration::from_secs(seconds))
}

/// Somewhere from half of `backoff` to all of it, so tasks that failed together don't
/// all retry at the same moment.
fn jittered(backoff: Duration) -> Duration {
    // A new RandomState is randomly seeded, which is all the randomness needed here.
    let random = RandomState::new().hash_one(0u8);
    backoff / 2 + backoff.mul_f64((random % 1000) as f64 / 2000.0)
}

/// Whether a failed connection failed on TLS, e.g. an untrusted certificate. The
/// rustls error comes wrapped in I/O errors, which hide what they wrap from `source`.
fn is_tls_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut next = Some(error);
    while let Some(error) = next {
        if error.is::<rustls::Error>() {
            return true;
        }
        next = match error.downcast_ref::<std::io::Error>() {
            Some(io) => io
                .get_ref()
                .map(|inner| inner as &(dyn std::error::Error + 'static)),
            None => error.source(),
        };
    }
    false
}

/// The host of a URL, for error messages (never the path or query, which may hold
/// tokens).
pub(crate) fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?']).next().unwrap_or(rest)
}

/// Percent-encodes a URL path segment or query value.
pub(crate) fn encode(value: &str) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
    /// Everything but the unreserved characters of RFC 3986.
    const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(value, COMPONENT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_leaves_out_path_and_query() {
        assert_eq!(host("https://example.com/b/x?token=secret"), "example.com");
        assert_eq!(host("https://example.com?sig=1"), "example.com");
    }

    #[test]
    fn invalid_urls_are_reported_without_their_query() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let http = Http::new().unwrap();
        let url = "https://acct.blob.core.windows.net/c?comp=list&sig=sec ret";
        let error = runtime.block_on(http.get(url, &[])).err().unwrap();
        let message = error.to_string();
        assert!(message.contains("acct.blob.core.windows.net"), "{message}");
        assert!(!message.contains("sec"), "{message}");
    }

    #[test]
    fn jitter_stays_between_half_and_all_of_the_backoff() {
        let backoff = Duration::from_secs(2);
        for _ in 0..100 {
            let wait = jittered(backoff);
            assert!(wait >= backoff / 2 && wait < backoff, "{wait:?}");
        }
    }

    #[test]
    fn reads_retry_after_seconds() {
        let response = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", value.parse().unwrap());
            Response {
                status: StatusCode::TOO_MANY_REQUESTS,
                headers,
                body: Bytes::new(),
            }
        };
        assert_eq!(retry_after(&response("3")), Some(Duration::from_secs(3)));
        assert_eq!(
            retry_after(&response("Wed, 21 Oct 2026 07:28:00 GMT")),
            None
        );
    }

    #[test]
    fn finds_tls_errors_inside_io_errors() {
        // How hyper-rustls reports an untrusted certificate: twice wrapped.
        let tls = std::io::Error::other(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::General("bad certificate".to_owned()),
        ));
        assert!(is_tls_error(&tls));
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert!(!is_tls_error(&refused));
    }

    #[test]
    fn encodes_reserved_characters() {
        assert_eq!(encode("logs/2024 a+b.txt"), "logs%2F2024%20a%2Bb.txt");
        assert_eq!(encode("plain-name_1.~"), "plain-name_1.~");
    }
}
