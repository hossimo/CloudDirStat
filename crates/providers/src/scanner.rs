use std::borrow::Cow;
use std::sync::Arc;

use clouddirstat_core::{Entry, Pricing};
use tokio::sync::mpsc::Sender;

use crate::azure::{AzureCredentials, AzureScanner};
use crate::gcs::{GcsCredentials, GcsScanner};
use crate::location::Location;
use crate::s3::{self, S3Scanner};
use crate::{Error, Result};

pub type EntrySender = Sender<Vec<Entry>>;

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub include_versions: bool,
    pub concurrency: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ScanStats {
    pub list_requests: u64,
    /// What the provider charges per 1,000 LIST requests.
    pub list_price_per_1000_usd: f64,
    /// Buckets left out of an all-buckets scan, e.g. for lack of permission to list them.
    pub skipped_buckets: Vec<SkippedBucket>,
    /// Optional checks that could not run, e.g. incomplete uploads without
    /// `s3:ListBucketMultipartUploads`. The scan itself still completed.
    pub warnings: Vec<String>,
}

impl ScanStats {
    pub fn estimated_cost_usd(&self) -> f64 {
        self.list_requests as f64 / 1000.0 * self.list_price_per_1000_usd
    }
}

#[derive(Clone, Debug)]
pub struct SkippedBucket {
    pub bucket: String,
    pub reason: String,
}

/// How to sign in at each provider. Only the one for the scanned location is used.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub aws: s3::CredentialSource,
    pub gcs: GcsCredentials,
    pub azure: AzureCredentials,
}

impl Default for Credentials {
    fn default() -> Self {
        Self {
            aws: s3::CredentialSource::Chain { profile: None },
            gcs: GcsCredentials::default(),
            azure: AzureCredentials::default(),
        }
    }
}

/// Lists a location at any provider.
pub enum Scanner {
    S3(S3Scanner),
    Gcs(GcsScanner),
    Azure(AzureScanner),
}

impl Scanner {
    /// Signs in and finds what to scan (for example each bucket's region).
    /// `region` stands in for region detection where S3 would look the region up (see
    /// [`S3Scanner::connect`]); other providers ignore it.
    pub async fn connect(
        location: &Location,
        credentials: &Credentials,
        region: Option<&str>,
    ) -> Result<Self> {
        match location {
            Location::S3(location) => S3Scanner::connect(location, &credentials.aws, region)
                .await
                .map(Self::S3),
            Location::Gcs(location) => GcsScanner::connect(location, &credentials.gcs)
                .await
                .map(Self::Gcs),
            Location::Azure(location) => AzureScanner::connect(location, &credentials.azure)
                .await
                .map(Self::Azure),
        }
    }

    /// Storage prices for what is being scanned.
    pub fn pricing(&self) -> Arc<dyn Pricing> {
        match self {
            Self::S3(scanner) => Arc::new(scanner.pricing()),
            Self::Gcs(scanner) => Arc::new(scanner.pricing()),
            Self::Azure(scanner) => Arc::new(scanner.pricing()),
        }
    }

    pub async fn scan(&self, options: &ScanOptions, sink: EntrySender) -> Result<ScanStats> {
        match self {
            Self::S3(scanner) => scanner.scan(options, sink).await,
            Self::Gcs(scanner) => scanner.scan(options, sink).await,
            Self::Azure(scanner) => scanner.scan(options, sink).await,
        }
    }

    /// Bucket totals without listing. Only S3 (from CloudWatch) so far.
    pub async fn estimate(&self) -> Result<s3::Estimate> {
        match self {
            Self::S3(scanner) => scanner.estimate().await,
            Self::Gcs(_) | Self::Azure(_) => Err(no_estimates()),
        }
    }

    /// Only the object counts of [`Scanner::estimate`], cheaply, for scan progress.
    pub async fn object_counts(&self) -> Result<s3::Estimate> {
        match self {
            Self::S3(scanner) => scanner.object_counts().await,
            Self::Gcs(_) | Self::Azure(_) => Err(no_estimates()),
        }
    }
}

fn no_estimates() -> Error {
    Error::Unsupported("estimates are only available for Amazon S3 so far".to_owned())
}

/// A storage class name, borrowed from the provider's `known` names when it is one, so
/// that most listed objects don't allocate a copy of it.
pub(crate) fn class_name(name: &str, known: &'static [&'static str]) -> Cow<'static, str> {
    match known.iter().find(|&&class| class == name) {
        Some(&class) => Cow::Borrowed(class),
        None => Cow::Owned(name.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_class_names_are_borrowed() {
        assert!(matches!(
            class_name("COLD", &["HOT", "COLD"]),
            Cow::Borrowed("COLD")
        ));
        assert!(matches!(class_name("NEW", &["HOT"]), Cow::Owned(name) if name == "NEW"));
    }
}
