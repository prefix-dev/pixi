/// Inspired from uv: https://github.com/astral-sh/uv/blob/ccdf2d793bbc2401c891b799772f615a28607e79/crates/uv-client/src/middleware.rs#L33
/// and used to verify that we don't do any requests during the tests.
use http::Extensions;
use std::fmt::Debug;

use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next};
use url::Url;

/// A custom error type for the offline middleware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OfflineError {
    url: Url,
}

impl OfflineError {
    /// Returns the URL that caused the error.
    pub(crate) fn url(&self) -> &Url {
        &self.url
    }
}

impl std::fmt::Display for OfflineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Network connectivity is disabled for `{}`", self.url)
    }
}

impl std::error::Error for OfflineError {}

/// A middleware that always returns an error indicating that the client is offline.
pub(crate) struct OfflineMiddleware;

#[async_trait::async_trait]
impl Middleware for OfflineMiddleware {
    async fn handle(
        &self,
        req: Request,
        _extensions: &mut Extensions,
        _next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        Err(reqwest_middleware::Error::Middleware(
            OfflineError {
                url: req.url().clone(),
            }
            .into(),
        ))
    }
}
