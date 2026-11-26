//! FIRST EPSS and CISA KEV clients and feed data.
//!
//! Each client uses caller-supplied middleware, TLS, and timeout configuration.
//! Feed data is independent of audit reports and risk-scoring policy.

use osv_client::http::{self, HttpFailure};
use thiserror::Error;
use url::Url;

mod epss;
mod kev;
mod types;

pub use epss::{DEFAULT_EPSS_BASE_URL, EPSS_BASE_URL_ENV_VAR, EpssClient};
pub use kev::{DEFAULT_KEV_URL, KEV_URL_ENV_VAR, KevCatalog, KevClient};
pub use types::{EpssScore, KevEntry, RansomwareCampaignUse};

/// A failed request to a threat-data feed (EPSS or KEV).
#[derive(Debug, Error)]
pub enum ThreatFeedError {
    #[error("failed to contact {url}")]
    Request {
        url: Url,
        #[source]
        source: reqwest_middleware::Error,
    },
    #[error(
        "{url} returned HTTP {status}{}",
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
    #[error("invalid threat data base URL")]
    InvalidBaseUrl(#[source] url::ParseError),
}

impl ThreatFeedError {
    fn from_http(url: Url, failure: HttpFailure) -> Self {
        match failure {
            HttpFailure::Request(source) => ThreatFeedError::Request { url, source },
            HttpFailure::Status { status, message } => ThreatFeedError::Status {
                url,
                status,
                message,
            },
            HttpFailure::InvalidResponse(source) => {
                ThreatFeedError::InvalidResponse { url, source }
            }
        }
    }
}
