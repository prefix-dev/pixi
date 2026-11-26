//! Distinct Basilisk and PyPI database clients and audit errors.

use thiserror::Error;
use url::Url;

mod basilisk;
mod pypi;

pub use basilisk::{
    BASILISK_URL_ENV_VAR, BasiliskClient, DEFAULT_BASILISK_URL, TOKEN_ENV_VAR, TOKEN_LENGTH,
    TOKEN_PREFIX, looks_like_basilisk_token,
};
pub use pypi::{DEFAULT_OSV_URL, OSV_URL_ENV_VAR, OsvDevClient};

/// A failed request to a vulnerability database.
#[derive(Debug, Error)]
pub enum AuditError {
    #[error("failed to contact the vulnerability database at {url}")]
    Request {
        url: Url,
        #[source]
        source: reqwest_middleware::Error,
    },
    #[error(
        "the vulnerability database at {url} returned HTTP {status}{}",
        .message.as_ref().map(|m| format!(": {m}")).unwrap_or_default()
    )]
    Status {
        url: Url,
        status: reqwest::StatusCode,
        /// The server's `{"error": "..."}` body, when present.
        message: Option<String>,
    },
    #[error("failed to parse the response from {url}")]
    InvalidResponse {
        url: Url,
        #[source]
        source: reqwest::Error,
    },
    #[error("unexpected response from {url}: expected {expected} results, got {got}")]
    UnexpectedResponse {
        url: Url,
        expected: usize,
        got: usize,
    },
    #[error("fetched vulnerability document {id} was not available")]
    MissingDocument { id: String },
    #[error("invalid audit API base URL")]
    InvalidBaseUrl(#[source] url::ParseError),
    #[error("the vulnerability database at {url} returned more than 32 result pages for one batch")]
    TooManyPages { url: Url },
}

impl From<osv_client::Error> for AuditError {
    fn from(error: osv_client::Error) -> Self {
        match error {
            osv_client::Error::Request { url, source } => Self::Request { url, source },
            osv_client::Error::Status {
                url,
                status,
                message,
            } => Self::Status {
                url,
                status,
                message,
            },
            osv_client::Error::InvalidResponse { url, source } => {
                Self::InvalidResponse { url, source }
            }
            osv_client::Error::UnexpectedResponse { url, expected, got } => {
                Self::UnexpectedResponse { url, expected, got }
            }
            osv_client::Error::InvalidBaseUrl(source) => Self::InvalidBaseUrl(source),
            osv_client::Error::TooManyPages { url } => Self::TooManyPages { url },
        }
    }
}

#[cfg(test)]
mod tests;
