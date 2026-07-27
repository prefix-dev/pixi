use miette::Diagnostic;
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use url::Url;

use crate::types::{BatchResponse, BatchResult, OsvQuery, OsvVulnerability};

/// The default basilisk API instance.
pub const DEFAULT_BASE_URL: &str = "https://api.basilisk.prefix.dev";

/// Environment variable to override the audit API base URL (tests,
/// self-hosted basilisk instances).
pub const BASE_URL_ENV_VAR: &str = "PIXI_AUDIT_BASE_URL";

/// Maximum number of queries per `querybatch` request (basilisk/OSV limit).
const MAX_BATCH_SIZE: usize = 1000;

#[derive(Debug, Error, Diagnostic)]
pub enum AuditError {
    #[error("failed to contact the vulnerability database at {url}")]
    Request {
        url: Url,
        #[source]
        source: reqwest_middleware::Error,
    },
    #[error("the vulnerability database at {url} returned HTTP {status}")]
    Status {
        url: Url,
        status: reqwest::StatusCode,
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
    #[error("invalid audit API base URL")]
    InvalidBaseUrl(#[source] url::ParseError),
}

/// Client for an OSV-protocol vulnerability API (basilisk).
pub struct BasiliskClient {
    client: ClientWithMiddleware,
    base_url: Url,
}

impl BasiliskClient {
    pub fn new(client: ClientWithMiddleware, base_url: Url) -> Self {
        Self { client, base_url }
    }

    fn endpoint(&self, path: &str) -> Result<Url, AuditError> {
        self.base_url.join(path).map_err(AuditError::InvalidBaseUrl)
    }

    /// Queries the database for all `queries`, in order. Handles chunking
    /// (max 1000 queries per request) and pagination transparently.
    pub async fn query_batch(&self, queries: &[OsvQuery]) -> Result<Vec<BatchResult>, AuditError> {
        let mut all_results = Vec::with_capacity(queries.len());
        for chunk in queries.chunks(MAX_BATCH_SIZE) {
            let mut results = self.query_batch_once(chunk).await?;

            // Follow pagination: re-send only the queries whose result
            // carried a `next_page_token`, until none are left.
            loop {
                let pending: Vec<(usize, OsvQuery)> = results
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, result)| {
                        result.next_page_token.as_ref().map(|token| {
                            let mut query = chunk[idx].clone();
                            query.page_token = Some(token.clone());
                            (idx, query)
                        })
                    })
                    .collect();
                if pending.is_empty() {
                    break;
                }
                let follow_up: Vec<OsvQuery> = pending.iter().map(|(_, q)| q.clone()).collect();
                let follow_up_results = self.query_batch_once(&follow_up).await?;
                for ((idx, _), extra) in pending.into_iter().zip(follow_up_results) {
                    results[idx].vulns.extend(extra.vulns);
                    results[idx].next_page_token = extra.next_page_token;
                }
            }

            all_results.extend(results);
        }
        Ok(all_results)
    }

    async fn query_batch_once(&self, queries: &[OsvQuery]) -> Result<Vec<BatchResult>, AuditError> {
        let url = self.endpoint("v1/querybatch")?;
        let response = self
            .client
            .post(url.clone())
            .json(&serde_json::json!({ "queries": queries }))
            .send()
            .await
            .map_err(|source| AuditError::Request {
                url: url.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(AuditError::Status { url, status });
        }
        let body: BatchResponse =
            response
                .json()
                .await
                .map_err(|source| AuditError::InvalidResponse {
                    url: url.clone(),
                    source,
                })?;
        if body.results.len() != queries.len() {
            return Err(AuditError::UnexpectedResponse {
                url,
                expected: queries.len(),
                got: body.results.len(),
            });
        }
        Ok(body.results)
    }

    /// Fetches the full OSV document for a vulnerability id.
    pub async fn get_vuln(&self, id: &str) -> Result<OsvVulnerability, AuditError> {
        let url = self.endpoint(&format!("v1/vulns/{id}"))?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|source| AuditError::Request {
                url: url.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(AuditError::Status { url, status });
        }
        response
            .json()
            .await
            .map_err(|source| AuditError::InvalidResponse { url, source })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Json, Router,
        extract::State,
        routing::{get, post},
    };
    use url::Url;

    use super::*;
    use crate::types::*;

    async fn spawn(app: Router) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Url::parse(&format!("http://{addr}/")).unwrap()
    }

    fn client(base_url: Url) -> BasiliskClient {
        let http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        BasiliskClient::new(http, base_url)
    }

    fn query(name: &str, version: &str) -> OsvQuery {
        OsvQuery {
            package: QueryPackage {
                name: Some(name.to_string()),
                ecosystem: Some("conda-forge".to_string()),
                purl: None,
            },
            version: Some(version.to_string()),
            page_token: None,
        }
    }

    #[tokio::test]
    async fn query_batch_chunks_requests_of_1000() {
        // Record the number of queries in each incoming request.
        let sizes: Arc<Mutex<Vec<usize>>> = Arc::default();
        let sizes_clone = sizes.clone();
        let app = Router::new()
            .route(
                "/v1/querybatch",
                post(
                    move |State(sizes): State<Arc<Mutex<Vec<usize>>>>,
                          Json(body): Json<serde_json::Value>| async move {
                        let n = body["queries"].as_array().unwrap().len();
                        sizes.lock().unwrap().push(n);
                        let results: Vec<serde_json::Value> =
                            (0..n).map(|_| serde_json::json!({})).collect();
                        Json(serde_json::json!({ "results": results }))
                    },
                ),
            )
            .with_state(sizes_clone);
        let base = spawn(app).await;

        let queries: Vec<OsvQuery> = (0..1001)
            .map(|i| query(&format!("pkg{i}"), "1.0"))
            .collect();
        let results = client(base).query_batch(&queries).await.unwrap();

        assert_eq!(results.len(), 1001);
        assert_eq!(*sizes.lock().unwrap(), vec![1000, 1]);
    }

    #[tokio::test]
    async fn query_batch_follows_pagination_tokens() {
        // First request: query 0 returns one vuln and a page token, query 1 is clean.
        // Second request (only the paged query): returns the second vuln, no token.
        let app = Router::new().route(
            "/v1/querybatch",
            post(|Json(body): Json<serde_json::Value>| async move {
                let queries = body["queries"].as_array().unwrap();
                let paged = queries
                    .first()
                    .and_then(|q| q.get("page_token"))
                    .is_some();
                if paged {
                    assert_eq!(queries.len(), 1);
                    Json(serde_json::json!({
                        "results": [{"vulns": [{"id": "BSLK-2", "modified": null}]}]
                    }))
                } else {
                    Json(serde_json::json!({
                        "results": [
                            {"vulns": [{"id": "BSLK-1", "modified": null}], "next_page_token": "tok"},
                            {}
                        ]
                    }))
                }
            }),
        );
        let base = spawn(app).await;

        let queries = vec![query("openssl", "3.1.0"), query("zlib", "1.3")];
        let results = client(base).query_batch(&queries).await.unwrap();

        let ids: Vec<&str> = results[0].vulns.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["BSLK-1", "BSLK-2"]);
        assert!(results[1].vulns.is_empty());
    }

    #[tokio::test]
    async fn get_vuln_fetches_full_document() {
        let app = Router::new().route(
            "/v1/vulns/{id}",
            get(
                |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    Json(serde_json::json!({"id": id, "aliases": ["CVE-2026-1234"]}))
                },
            ),
        );
        let base = spawn(app).await;

        let vuln = client(base).get_vuln("BSLK-1").await.unwrap();
        assert_eq!(vuln.id, "BSLK-1");
        assert_eq!(vuln.aliases, vec!["CVE-2026-1234"]);
    }

    #[tokio::test]
    async fn server_error_is_reported() {
        let app = Router::new().route(
            "/v1/querybatch",
            post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        );
        let base = spawn(app).await;

        let err = client(base)
            .query_batch(&[query("openssl", "3.1.0")])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("500"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn mismatched_result_count_is_an_error() {
        let app = Router::new().route(
            "/v1/querybatch",
            post(|| async { Json(serde_json::json!({"results": []})) }),
        );
        let base = spawn(app).await;

        let err = client(base)
            .query_batch(&[query("openssl", "3.1.0")])
            .await
            .unwrap_err();
        assert!(
            matches!(err, AuditError::UnexpectedResponse { .. }),
            "unexpected error: {err}"
        );
    }
}
