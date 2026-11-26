//! OSV-protocol HTTP transport with ordered batching and bounded pagination.
//!
//! Callers supply a middleware client and choose their own TLS, timeout, and
//! retry configuration. This crate does not select a vulnerability database.

use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use serde::de::DeserializeOwned;
use thiserror::Error;
use url::Url;

use http::HttpFailure;
use osv_protocol::{BatchResponse, BatchResult, OsvQuery, OsvVulnerability};

pub mod http;

const MAX_BATCH_SIZE: usize = 1000;
const MAX_PAGINATION_ROUNDS: usize = 32;

/// A failed request to a vulnerability database.
#[derive(Debug, Error)]
pub enum Error {
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
    #[error("unexpected response from {url}: expected {expected} results, got {got}")]
    UnexpectedResponse {
        url: Url,
        expected: usize,
        got: usize,
    },
    #[error("invalid OSV API base URL")]
    InvalidBaseUrl(#[source] url::ParseError),
    #[error("{url} returned more than {MAX_PAGINATION_ROUNDS} result pages for one batch")]
    TooManyPages { url: Url },
}

impl Error {
    fn from_http(url: Url, failure: HttpFailure) -> Self {
        match failure {
            HttpFailure::Request(source) => Error::Request { url, source },
            HttpFailure::Status { status, message } => Error::Status {
                url,
                status,
                message,
            },
            HttpFailure::InvalidResponse(source) => Error::InvalidResponse { url, source },
        }
    }
}

/// Client for an OSV-protocol vulnerability API.
pub struct OsvClient {
    client: ClientWithMiddleware,
    base_url: Url,
}

impl OsvClient {
    /// Uses the caller's middleware client, including its TLS and timeout policy.
    pub fn new(client: ClientWithMiddleware, base_url: Url) -> Self {
        Self {
            client,
            base_url: http::base_url(base_url),
        }
    }

    fn endpoint(&self, path: &str) -> Result<Url, Error> {
        self.base_url.join(path).map_err(Error::InvalidBaseUrl)
    }

    /// Queries the database for all `queries`, in order. Handles chunking
    /// (max 1000 queries per request) and pagination transparently.
    pub async fn query_batch(&self, queries: &[OsvQuery]) -> Result<Vec<BatchResult>, Error> {
        let mut all_results = Vec::with_capacity(queries.len());
        for chunk in queries.chunks(MAX_BATCH_SIZE) {
            let mut results = self.query_batch_once(chunk).await?;

            // Follow pagination: re-send only the queries whose result
            // carried a `next_page_token`, until none are left. Bounded by
            // `MAX_PAGINATION_ROUNDS` so a server that always returns a
            // token can't make this loop forever.
            for round in 0.. {
                let mut pending_indices = Vec::new();
                let mut follow_up = Vec::new();
                for (idx, result) in results.iter().enumerate() {
                    if let Some(token) = &result.next_page_token {
                        let mut query = chunk[idx].clone();
                        query.page_token = Some(token.clone());
                        pending_indices.push(idx);
                        follow_up.push(query);
                    }
                }
                if follow_up.is_empty() {
                    break;
                }
                if round >= MAX_PAGINATION_ROUNDS {
                    return Err(Error::TooManyPages {
                        url: self.endpoint("v1/querybatch")?,
                    });
                }
                let follow_up_results = self.query_batch_once(&follow_up).await?;
                for (idx, extra) in pending_indices.into_iter().zip(follow_up_results) {
                    results[idx].vulns.extend(extra.vulns);
                    results[idx].next_page_token = extra.next_page_token;
                }
            }

            all_results.extend(results);
        }
        Ok(all_results)
    }

    async fn query_batch_once(&self, queries: &[OsvQuery]) -> Result<Vec<BatchResult>, Error> {
        let url = self.endpoint("v1/querybatch")?;
        let request = self
            .client
            .post(url.clone())
            .json(&serde_json::json!({ "queries": queries }));
        let body: BatchResponse = self.send_json(url.clone(), request).await?;
        if body.results.len() != queries.len() {
            return Err(Error::UnexpectedResponse {
                url,
                expected: queries.len(),
                got: body.results.len(),
            });
        }
        Ok(body.results)
    }

    /// Fetches the full OSV document for a vulnerability id.
    pub async fn get_vuln(&self, id: &str) -> Result<OsvVulnerability, Error> {
        let mut url = self.endpoint("v1/vulns")?;
        // Push the id as a single percent-encoded segment so an id like
        // `../health` cannot redirect the fetch to a different path.
        url.path_segments_mut()
            .expect("an HTTP(S) URL can always be a base")
            .push(id);
        let request = self.client.get(url.clone());
        self.send_json(url, request).await
    }

    /// Sends `request` to `url` and decodes a successful JSON response.
    async fn send_json<T: DeserializeOwned>(
        &self,
        url: Url,
        request: RequestBuilder,
    ) -> Result<T, Error> {
        http::send_json(request)
            .await
            .map_err(|failure| Error::from_http(url, failure))
    }
}

#[cfg(test)]
mod tests;
