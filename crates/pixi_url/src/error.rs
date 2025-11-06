use rattler_digest::{Md5Hash, Sha256Hash};
use reqwest::StatusCode;
use reqwest_middleware::Error as ReqwestMiddlewareError;
use thiserror::Error;
use url::Url;

/// Errors that can occur while fetching and unpacking a URL source.
#[derive(Debug, Error)]
pub enum UrlError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("failed to download {url}: {status}")]
    HttpStatus { url: Url, status: StatusCode },

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    #[error(transparent)]
    ReqwestMiddleware(#[from] ReqwestMiddlewareError),

    #[error("sha256 mismatch for {url}: expected {expected:x}, got {actual:x}")]
    Sha256Mismatch {
        url: Url,
        expected: Sha256Hash,
        actual: Sha256Hash,
    },

    #[error("md5 mismatch for {url}: expected {expected:x}, got {actual:x}")]
    Md5Mismatch {
        url: Url,
        expected: Md5Hash,
        actual: Md5Hash,
    },

    #[error(transparent)]
    Extract(#[from] ExtractError),

    #[error("unsupported archive format: {0}")]
    UnsupportedArchive(String),

    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

/// Errors emitted while unpacking an archive.
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("failed to extract tar archive: {0}")]
    TarExtractionError(String),

    #[error("failed to extract zip archive: {0}")]
    ZipExtractionError(String),

    #[error("invalid zip archive: {0}")]
    InvalidZip(String),

    #[error("failed to extract 7z archive: {0}")]
    SevenZipExtractionError(String),

    #[error("compression format `{0}` is currently unsupported")]
    UnsupportedCompression(&'static str),
}
