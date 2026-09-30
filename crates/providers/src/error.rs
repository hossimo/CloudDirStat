#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{operation} failed (requires {permission}): {message}")]
    Request {
        operation: &'static str,
        permission: &'static str,
        message: String,
    },

    #[error("could not sign in (see Troubleshooting in README.md): {0}")]
    Credentials(String),

    #[error(
        "invalid location {0:?}, expected s3://bucket/prefix, gs://bucket/prefix, or az://account/container/prefix (s3://, gs://, or az:// alone for everything)"
    )]
    InvalidLocation(String),

    #[error(
        "invalid name {0:?}: bucket and container names are 3 to 63 lowercase letters, numbers, dots, and hyphens; storage account names 3 to 24 lowercase letters and numbers"
    )]
    InvalidBucketName(String),

    #[error("bucket {0} does not exist")]
    NoSuchBucket(String),

    #[error("{0}")]
    Network(String),

    #[error("{0}")]
    Unsupported(String),

    #[error("scan was cancelled")]
    Cancelled,

    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}
