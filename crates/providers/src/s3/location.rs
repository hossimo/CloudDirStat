use std::fmt;
use std::str::FromStr;

use crate::Error;

/// `s3://bucket/prefix`, or `s3://` for every bucket the credentials can list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Location {
    /// Empty for all buckets.
    pub bucket: String,
    pub prefix: String,
}

impl S3Location {
    pub fn all_buckets() -> Self {
        Self {
            bucket: String::new(),
            prefix: String::new(),
        }
    }

    pub fn is_all_buckets(&self) -> bool {
        self.bucket.is_empty()
    }
}

impl FromStr for S3Location {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        if input == "s3://" {
            return Ok(Self::all_buckets());
        }
        let path = input.strip_prefix("s3://").unwrap_or(input);
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

/// Whether `name` could be a bucket. Current rules allow 3 to 63 lowercase letters,
/// digits, dots, and hyphens; older us-east-1 buckets may also be up to 255 characters
/// with capitals and underscores, so those are let through too.
fn is_bucket_name(name: &str) -> bool {
    (3..=255).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

impl fmt::Display for S3Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_all_buckets() {
            return write!(f, "s3://");
        }
        write!(f, "s3://{}/{}", self.bucket, self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> (String, String) {
        let location: S3Location = input.parse().unwrap();
        (location.bucket, location.prefix)
    }

    #[test]
    fn parses_bucket_and_prefix() {
        assert_eq!(parse("s3://bucket"), ("bucket".into(), "".into()));
        assert_eq!(parse("s3://bucket/"), ("bucket".into(), "".into()));
        assert_eq!(
            parse("s3://bucket/logs/2024/"),
            ("bucket".into(), "logs/2024/".into())
        );
        assert_eq!(parse("bucket/logs"), ("bucket".into(), "logs".into()));
    }

    #[test]
    fn rejects_missing_bucket() {
        assert!("".parse::<S3Location>().is_err());
        assert!("/prefix".parse::<S3Location>().is_err());
        assert!("s3:///prefix".parse::<S3Location>().is_err());
    }

    #[test]
    fn rejects_impossible_bucket_names() {
        for input in ["s3://i", "s3://ab/", "s3://my bucket", "s3://bucket?/x"] {
            assert!(
                matches!(
                    input.parse::<S3Location>(),
                    Err(Error::InvalidBucketName(_))
                ),
                "{input}"
            );
        }
        assert!("s3://Legacy_Bucket".parse::<S3Location>().is_ok());
    }

    #[test]
    fn bare_scheme_means_all_buckets() {
        let location: S3Location = "s3://".parse().unwrap();
        assert!(location.is_all_buckets());
        assert_eq!(location.to_string(), "s3://");
    }
}
