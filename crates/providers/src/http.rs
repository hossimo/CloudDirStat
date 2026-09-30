//! A small HTTPS client for the providers that are called over plain REST (Google Cloud
//! Storage and Azure), built on the same hyper and rustls the AWS SDK already brings in.

use std::time::Duration;

use http::{Method, Request, StatusCode};
use http_body_util::{BodyExt, Full};
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

#[derive(Clone)]
pub(crate) struct Http {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

pub(crate) struct Response {
    pub status: StatusCode,
    pub body: Bytes,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
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
    /// exponential backoff. Other responses (including 4xx errors) are returned as is.
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
            let request = request
                .body(Full::new(body.clone()))
                .map_err(|error| Error::Network(format!("invalid request to {url}: {error}")))?;

            let result = tokio::time::timeout(TIMEOUT, self.round_trip(request)).await;
            let retry = match &result {
                Ok(Ok(response)) => {
                    response.status == StatusCode::TOO_MANY_REQUESTS
                        || response.status.is_server_error()
                }
                Ok(Err(_)) | Err(_) => true,
            };
            if !retry || attempt == ATTEMPTS {
                return match result {
                    Ok(result) => result,
                    Err(_) => Err(Error::Network(format!(
                        "no response from {} within {}s",
                        host(url),
                        TIMEOUT.as_secs()
                    ))),
                };
            }
            tokio::time::sleep(backoff).await;
            backoff *= 2;
            attempt += 1;
        }
    }

    async fn round_trip(&self, request: Request<Full<Bytes>>) -> Result<Response> {
        let host = host(&request.uri().to_string()).to_owned();
        let response = self
            .client
            .request(request)
            .await
            .map_err(|error| Error::Network(format!("could not reach {host}: {error}")))?;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| Error::Network(format!("connection to {host} failed: {error}")))?
            .to_bytes();
        Ok(Response { status, body })
    }
}

/// The host of a URL, for error messages (never the path or query, which may hold
/// tokens).
fn host(url: &str) -> &str {
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
    fn encodes_reserved_characters() {
        assert_eq!(encode("logs/2024 a+b.txt"), "logs%2F2024%20a%2Bb.txt");
        assert_eq!(encode("plain-name_1.~"), "plain-name_1.~");
    }
}
