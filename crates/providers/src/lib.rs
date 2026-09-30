pub mod azure;
mod error;
pub mod gcs;
mod http;
mod location;
pub mod s3;
mod scanner;
mod time;
mod xml;

pub use error::Error;
pub use location::{Location, Provider};
pub use scanner::{Credentials, EntrySender, ScanOptions, ScanStats, Scanner, SkippedBucket};

pub type Result<T> = std::result::Result<T, Error>;
