use std::fmt;
use std::str::FromStr;

use crate::Error;

/// `gs://bucket/prefix`, or `gs://` for every bucket of a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcsLocation {
    /// Empty for all buckets.
    pub bucket: String,
    pub prefix: String,
}

impl GcsLocation {
    pub fn is_all_buckets(&self) -> bool {
        self.bucket.is_empty()
    }
}

impl FromStr for GcsLocation {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let path = input
            .strip_prefix("gs://")
            .ok_or_else(|| Error::InvalidLocation(input.to_owned()))?;
        if path.is_empty() {
            return Ok(Self {
                bucket: String::new(),
                prefix: String::new(),
            });
        }
        let (bucket, prefix) = path.split_once('/').unwrap_or((path, ""));
        if bucket.is_empty() {
            return Err(Error::InvalidLocation(input.to_owned()));
        }
        if !is_bucket_name(bucket) {
            return Err(Error::InvalidBucketName(bucket.to_owned()));
        }
        Ok(Self {
            bucket: bucket.to_owned(),
            prefix: prefix.to_owned(),
        })
    }
}

/// Cloud Storage bucket names: 3 to 63 characters (222 with dots) of lowercase letters,
/// digits, dots, hyphens, and underscores, starting and ending with a letter or digit.
fn is_bucket_name(name: &str) -> bool {
    let alphanumeric = |byte: Option<u8>| byte.is_some_and(|byte| byte.is_ascii_alphanumeric());
    (3..=222).contains(&name.len())
        && alphanumeric(name.bytes().next())
        && alphanumeric(name.bytes().last())
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
}

impl fmt::Display for GcsLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_all_buckets() {
            return write!(f, "gs://");
        }
        write!(f, "gs://{}/{}", self.bucket, self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bucket_prefix_and_all_buckets() {
        let location: GcsLocation = "gs://my-bucket/logs/2024/".parse().unwrap();
        assert_eq!(location.bucket, "my-bucket");
        assert_eq!(location.prefix, "logs/2024/");
        assert_eq!(location.to_string(), "gs://my-bucket/logs/2024/");

        let all: GcsLocation = "gs://".parse().unwrap();
        assert!(all.is_all_buckets());
        assert_eq!(all.to_string(), "gs://");
    }

    #[test]
    fn rejects_impossible_bucket_names() {
        for input in [
            "gs://ab",
            "gs://My-Bucket",
            "gs://-bucket",
            "gs://bucket-/x",
        ] {
            assert!(
                matches!(
                    input.parse::<GcsLocation>(),
                    Err(Error::InvalidBucketName(_))
                ),
                "{input}"
            );
        }
        assert!("gs:///prefix".parse::<GcsLocation>().is_err());
    }
}
