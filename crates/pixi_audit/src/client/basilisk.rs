use osv_client::OsvClient;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use super::AuditError;
use crate::types::{BatchResult, OsvQuery, OsvVulnerability};

/// The default Basilisk API instance.
pub const DEFAULT_BASILISK_URL: &str = "https://api.basilisk.prefix.dev";

/// Environment variable to override the audit API base URL.
pub const BASILISK_URL_ENV_VAR: &str = "PIXI_AUDIT_BASILISK_URL";

/// Environment variable carrying a Basilisk user API token.
pub const TOKEN_ENV_VAR: &str = "PIXI_AUDIT_TOKEN";

/// Visible prefix of a Basilisk user API token.
pub const TOKEN_PREFIX: &str = "bsk_";

/// Full token length: the prefix, an 8-character key id and a 32-character secret.
pub const TOKEN_LENGTH: usize = 44;

/// Checks token shape, not validity.
pub fn looks_like_basilisk_token(value: &str) -> bool {
    value.len() == TOKEN_LENGTH
        && value.starts_with(TOKEN_PREFIX)
        && value[TOKEN_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric())
}

/// Client for the Basilisk vulnerability database.
pub struct BasiliskClient {
    inner: OsvClient,
}

impl BasiliskClient {
    /// Uses the caller's middleware, TLS, and timeout configuration.
    pub fn new(client: ClientWithMiddleware, base_url: Url) -> Self {
        Self {
            inner: OsvClient::new(client, base_url),
        }
    }

    /// Queries the database in order, handling chunking and pagination.
    pub async fn query_batch(&self, queries: &[OsvQuery]) -> Result<Vec<BatchResult>, AuditError> {
        self.inner.query_batch(queries).await.map_err(Into::into)
    }

    /// Fetches the full OSV document for a vulnerability id.
    pub async fn get_vuln(&self, id: &str) -> Result<OsvVulnerability, AuditError> {
        self.inner.get_vuln(id).await.map_err(Into::into)
    }

    pub(crate) fn osv_client(&self) -> &OsvClient {
        &self.inner
    }
}
