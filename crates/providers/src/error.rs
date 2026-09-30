#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{operation} failed (requires {permission}): {message}")]
    Request {
        operation: &'static str,
        permission: &'static str,
        message: String,
    },

    #[error("could not load AWS credentials (see Troubleshooting in README.md): {0}")]
    Credentials(String),

    #[error(
        "invalid location {0:?}, expected s3://bucket, s3://bucket/prefix, or s3:// for all buckets"
    )]
    InvalidLocation(String),

    #[error(
        "invalid bucket name {0:?}: bucket names are 3 to 63 lowercase letters, numbers, dots, and hyphens"
    )]
    InvalidBucketName(String),

    #[error("bucket {0} does not exist")]
    NoSuchBucket(String),

    #[error("scan was cancelled")]
    Cancelled,

    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}
