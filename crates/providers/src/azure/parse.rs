//! Reading the Blob service's XML responses: pages of blobs and containers, and errors.

use std::borrow::Cow;

use crate::Error;
use crate::http::Response;
use crate::scanner::class_name;
use crate::time::http_date_seconds;
use crate::xml::{XmlEvent, walk};

/// The access tiers Azure lists blobs in.
const TIERS: &[&str] = &["Hot", "Cool", "Cold", "Archive", "Premium"];

#[derive(Debug, Default, PartialEq)]
pub(super) struct Blob {
    pub name: String,
    pub deleted: bool,
    pub snapshot: bool,
    pub version: bool,
    pub current_version: bool,
    pub size: u64,
    pub tier: Option<Cow<'static, str>>,
    /// Seconds since the Unix epoch.
    pub last_modified: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct BlobPage {
    pub blobs: Vec<Blob>,
    pub prefixes: Vec<String>,
    pub next_marker: Option<String>,
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

pub(super) fn parse_blobs(xml: &str) -> std::result::Result<BlobPage, String> {
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

/// One page of a container listing.
#[derive(Debug, Default)]
pub(super) struct ContainerPage {
    pub names: Vec<String>,
    pub next_marker: Option<String>,
}

pub(super) fn parse_containers(xml: &str) -> std::result::Result<ContainerPage, String> {
    let mut page = ContainerPage::default();
    walk(xml, |event| {
        if let XmlEvent::End(path, text, _) = event {
            match path {
                [.., item, name] if item == "Container" && name == "Name" => {
                    page.names.push(text.to_owned());
                }
                [root, marker] if root == "EnumerationResults" && marker == "NextMarker" => {
                    page.next_marker = next_marker(text);
                }
                _ => {}
            }
        }
    })?;
    Ok(page)
}

pub(super) fn bad_xml(error: String) -> Error {
    Error::Network(format!("unexpected Blob Storage response: {error}"))
}

/// `Code: Message` of a Blob service error body, or the HTTP status.
pub(super) fn azure_error(response: &Response) -> String {
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
    fn parses_container_pages() {
        let xml = r#"<EnumerationResults><Containers><Container><Name>logs</Name></Container><Container><Name>photos</Name></Container></Containers><NextMarker/></EnumerationResults>"#;
        let page = parse_containers(xml).unwrap();
        assert_eq!(page.names, ["logs", "photos"]);
        assert_eq!(page.next_marker, None);
    }
}
