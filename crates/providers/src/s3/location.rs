use std::fmt;
use std::str::FromStr;

use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Location {
    pub bucket: String,
    pub prefix: String,
}

impl FromStr for S3Location {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let path = input.strip_prefix("s3://").unwrap_or(input);
        let (bucket, prefix) = path.split_once('/').unwrap_or((path, ""));
        if bucket.is_empty() {
            return Err(Error::InvalidLocation(input.to_owned()));
        }
        Ok(Self {
            bucket: bucket.to_owned(),
            prefix: prefix.to_owned(),
        })
    }
}

impl fmt::Display for S3Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        assert!("s3://".parse::<S3Location>().is_err());
        assert!("/prefix".parse::<S3Location>().is_err());
    }
}
