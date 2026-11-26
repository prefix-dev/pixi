use osv_client::OsvClient;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

/// The default OSV API used to audit PyPI packages.
pub const DEFAULT_OSV_URL: &str = "https://api.osv.dev";

/// Environment variable to override the PyPI vulnerability API.
pub const OSV_URL_ENV_VAR: &str = "PIXI_AUDIT_OSV_URL";

/// Client for auditing PyPI packages against the OSV API.
///
/// A distinct type from [`super::BasiliskClient`] so the two databases cannot be
/// swapped by accident, even though they speak the same protocol.
pub struct OsvDevClient {
    inner: OsvClient,
}

impl OsvDevClient {
    /// Wraps `client`, which carries whatever middleware, timeouts, and
    /// proxy configuration the caller set up. The same client can be shared
    /// with a [`super::BasiliskClient`]. No timeout is applied here, so callers
    /// should configure one on `client`.
    pub fn new(client: ClientWithMiddleware, base_url: Url) -> Self {
        Self {
            inner: OsvClient::new(client, base_url),
        }
    }

    /// The OSV-protocol client this wraps.
    pub(crate) fn osv_client(&self) -> &OsvClient {
        &self.inner
    }
}
