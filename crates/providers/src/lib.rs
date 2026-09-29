mod error;
pub mod s3;

pub use error::Error;

pub type Result<T> = std::result::Result<T, Error>;
