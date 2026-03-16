use rattler_digest::{Md5Hash, Sha256Hash};
use reqwest::StatusCode;
use reqwest_middleware::Error as ReqwestMiddlewareError;
use std::sync::Arc;
use thiserror::Error;
use url::Url;

/// Errors that can occur while fetching and unpacking a URL source.
#[derive(Debug, Clone, Error)]
pub enum UrlError {
    #[error(transparent)]
    Io(Arc<std::io::Error>),

    #[error("failed to download {url}: {status}")]
    HttpStatus { url: Url, status: StatusCode },

    #[error(transparent)]
    Reqwest(Arc<reqwest::Error>),

    #[error(transparent)]
    ReqwestMiddleware(Arc<ReqwestMiddlewareError>),

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
    Join(Arc<tokio::task::JoinError>),
}

impl From<std::io::Error> for UrlError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(Arc::new(err))
    }
}

impl From<reqwest::Error> for UrlError {
    fn from(err: reqwest::Error) -> Self {
        Self::Reqwest(Arc::new(err))
    }
}

impl From<ReqwestMiddlewareError> for UrlError {
    fn from(err: ReqwestMiddlewareError) -> Self {
        Self::ReqwestMiddleware(Arc::new(err))
    }
}

impl From<tokio::task::JoinError> for UrlError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Join(Arc::new(err))
    }
}

/// Errors emitted while unpacking an archive.
#[derive(Debug, Clone, Error)]
pub enum ExtractError {
    #[error(transparent)]
    Io(Arc<std::io::Error>),

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

impl From<std::io::Error> for ExtractError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(Arc::new(err))
    }
}
