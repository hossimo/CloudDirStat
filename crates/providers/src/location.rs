use std::fmt;
use std::str::FromStr;

use clouddirstat_core::EntryKind;

use crate::Error;
use crate::azure::AzureLocation;
use crate::gcs::GcsLocation;
use crate::s3::S3Location;

/// A cloud storage provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    S3,
    Gcs,
    Azure,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Self::S3, Self::Gcs, Self::Azure];

    /// The start of this provider's locations: `s3://`, `gs://`, or `az://`.
    pub fn scheme(self) -> &'static str {
        match self {
            Self::S3 => "s3://",
            Self::Gcs => "gs://",
            Self::Azure => "az://",
        }
    }

    /// The provider whose scheme `scheme` is, without the `://` (`"s3"`, `"gs"`, `"az"`).
    pub fn from_scheme(scheme: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.scheme().strip_suffix("://") == Some(scheme))
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::S3 => "Amazon S3",
            Self::Gcs => "Google Cloud Storage",
            Self::Azure => "Azure Blob Storage",
        }
    }

    /// What scanning with versions adds, in this provider's terms.
    pub fn versions_help(self) -> &'static str {
        match self {
            Self::S3 => "Include noncurrent versions and delete markers",
            Self::Gcs => "Include noncurrent versions",
            Self::Azure => "Include previous versions, snapshots, and soft-deleted blobs",
        }
    }

    /// The kinds of entries this provider's scans can find; only S3 has delete markers
    /// and incomplete multipart uploads.
    pub fn entry_kinds(self) -> &'static [EntryKind] {
        match self {
            Self::S3 => &EntryKind::ALL,
            Self::Gcs | Self::Azure => &[EntryKind::Current, EntryKind::Noncurrent],
        }
    }
}

/// Where to scan: a bucket (or container) and prefix at one provider, or everything the
/// credentials can see there. Parsed from a URL such as `s3://bucket/prefix/`; text
/// without a scheme is taken as S3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    S3(S3Location),
    Gcs(GcsLocation),
    Azure(AzureLocation),
}

impl Location {
    pub fn provider(&self) -> Provider {
        match self {
            Self::S3(_) => Provider::S3,
            Self::Gcs(_) => Provider::Gcs,
            Self::Azure(_) => Provider::Azure,
        }
    }

    /// The prefix within the bucket; empty for a whole bucket.
    pub fn prefix(&self) -> &str {
        match self {
            Self::S3(location) => &location.prefix,
            Self::Gcs(location) => &location.prefix,
            Self::Azure(location) => &location.prefix,
        }
    }

    /// Whether this is every bucket the credentials can see (or every container of an
    /// Azure account), each shown as a top-level folder.
    pub fn is_all(&self) -> bool {
        match self {
            Self::S3(location) => location.is_all_buckets(),
            Self::Gcs(location) => location.is_all_buckets(),
            Self::Azure(location) => location.is_all_containers(),
        }
    }

    /// The bucket (or Azure container), unless this is every bucket.
    pub fn bucket(&self) -> Option<&str> {
        let bucket = match self {
            Self::S3(location) => &location.bucket,
            Self::Gcs(location) => &location.bucket,
            Self::Azure(location) => &location.container,
        };
        Some(bucket.as_str()).filter(|bucket| !bucket.is_empty())
    }
}

impl FromStr for Location {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let provider = match input.split_once("://") {
            None => Provider::S3,
            Some((scheme, _)) => Provider::from_scheme(scheme)
                .ok_or_else(|| Error::InvalidLocation(input.to_owned()))?,
        };
        match provider {
            Provider::S3 => input.parse().map(Self::S3),
            Provider::Gcs => input.parse().map(Self::Gcs),
            Provider::Azure => input.parse().map(Self::Azure),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::S3(location) => location.fmt(f),
            Self::Gcs(location) => location.fmt(f),
            Self::Azure(location) => location.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_by_scheme() {
        let location: Location = "s3://bucket/logs/".parse().unwrap();
        assert_eq!(location.provider(), Provider::S3);
        assert_eq!(location.bucket(), Some("bucket"));
        assert_eq!(location.prefix(), "logs/");
        assert_eq!(location.to_string(), "s3://bucket/logs/");

        let bare: Location = "bucket/logs".parse().unwrap();
        assert_eq!(bare.provider(), Provider::S3);

        let all: Location = "s3://".parse().unwrap();
        assert!(all.is_all());
        assert_eq!(all.bucket(), None);

        let google: Location = "gs://photos/2024/".parse().unwrap();
        assert_eq!(google.provider(), Provider::Gcs);
        assert_eq!(google.bucket(), Some("photos"));
        assert_eq!(google.prefix(), "2024/");
        assert!("gs://".parse::<Location>().unwrap().is_all());

        let azure: Location = "az://acct/photos/2024/".parse().unwrap();
        assert_eq!(azure.provider(), Provider::Azure);
        assert_eq!(azure.bucket(), Some("photos"));
        assert_eq!(azure.prefix(), "2024/");
        assert!("az://acct".parse::<Location>().unwrap().is_all());
        assert!("az://".parse::<Location>().unwrap().is_all());
    }

    #[test]
    fn rejects_unknown_schemes() {
        assert!(matches!(
            "ftp://host/path".parse::<Location>(),
            Err(Error::InvalidLocation(_))
        ));
    }
}
