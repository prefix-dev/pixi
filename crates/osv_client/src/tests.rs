use std::sync::{Arc, Mutex};

use super::*;
use axum::{
    Json, Router,
    routing::{get, post},
};
use osv_protocol::*;
use pixi_audit_test_support::{osv_router, spawn_router as spawn};

fn client(base_url: Url) -> OsvClient {
    let http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
    OsvClient::new(http, base_url)
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
    let app = osv_router(
        move |queries| {
            sizes_clone.lock().unwrap().push(queries.len());
            BatchResponse {
                results: vec![BatchResult::default(); queries.len()],
            }
        },
        [],
    );
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
    let app = osv_router(
        |queries| {
            let paged = queries
                .first()
                .and_then(|query| query.page_token.as_ref())
                .is_some();
            let results = if paged {
                assert_eq!(queries.len(), 1);
                vec![BatchResult {
                    vulns: vec![VulnRef {
                        id: "BSLK-2".into(),
                        modified: None,
                    }],
                    next_page_token: None,
                }]
            } else {
                vec![
                    BatchResult {
                        vulns: vec![VulnRef {
                            id: "BSLK-1".into(),
                            modified: None,
                        }],
                        next_page_token: Some("tok".into()),
                    },
                    BatchResult::default(),
                ]
            };
            BatchResponse { results }
        },
        [],
    );
    let base = spawn(app).await;

    let queries = vec![query("openssl", "3.1.0"), query("zlib", "1.3")];
    let results = client(base).query_batch(&queries).await.unwrap();

    let ids: Vec<&str> = results[0].vulns.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, vec!["BSLK-1", "BSLK-2"]);
    assert!(results[1].vulns.is_empty());
}

#[tokio::test]
async fn query_batch_caps_unbounded_pagination() {
    // A buggy/malicious server that always returns a `next_page_token`
    // must not make `query_batch` loop forever.
    let app = osv_router(
        |queries| BatchResponse {
            results: vec![
                BatchResult {
                    next_page_token: Some("tok".into()),
                    ..BatchResult::default()
                };
                queries.len()
            ],
        },
        [],
    );
    let base = spawn(app).await;

    let err = client(base)
        .query_batch(&[query("openssl", "3.1.0")])
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::TooManyPages { .. }),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn get_vuln_percent_encodes_the_id() {
    // Ids come from the server's own querybatch response, but a hostile
    // or buggy server must not be able to redirect the detail fetch to a
    // different path via an id like `../health`.
    let app = osv_router(
        |_| BatchResponse { results: vec![] },
        [OsvVulnerability {
            id: "../health".into(),
            ..OsvVulnerability::default()
        }],
    )
    .route(
        "/health",
        get(|| async { Json(serde_json::json!({"id": "WRONG"})) }),
    );
    let base = spawn(app).await;

    let vuln = client(base).get_vuln("../health").await.unwrap();
    assert_eq!(vuln.id, "../health");
}

#[tokio::test]
async fn base_url_with_deep_path_keeps_all_segments() {
    let app = Router::new().nest(
        "/a/b",
        osv_router(
            |_| BatchResponse {
                results: vec![BatchResult::default()],
            },
            [],
        ),
    );
    let base = spawn(app).await;
    let deep = base.join("a/b").unwrap();
    assert!(!deep.path().ends_with('/'));

    let results = client(deep)
        .query_batch(&[query("openssl", "3.1.0")])
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn get_vuln_fetches_full_document() {
    let app = osv_router(
        |_| BatchResponse { results: vec![] },
        [OsvVulnerability {
            id: "BSLK-1".into(),
            aliases: vec!["CVE-2026-1234".into()],
            ..OsvVulnerability::default()
        }],
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
    assert!(matches!(
        err,
        Error::Status {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            message: None,
            ..
        }
    ));
}

#[tokio::test]
async fn server_error_body_message_is_surfaced() {
    // basilisk returns structured {"error": "..."} bodies on 4xx/5xx.
    let app = Router::new().route(
        "/v1/querybatch",
        post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "querybatch accepts at most 1000 queries"
                })),
            )
        }),
    );
    let base = spawn(app).await;

    let err = client(base)
        .query_batch(&[query("openssl", "3.1.0")])
        .await
        .unwrap_err();
    match err {
        Error::Status {
            status, message, ..
        } => {
            assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
            assert_eq!(
                message.as_deref(),
                Some("querybatch accepts at most 1000 queries")
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn malformed_response_is_an_error() {
    let app = Router::new().route("/v1/querybatch", post(|| async { "not JSON" }));
    let base = spawn(app).await;

    let err = client(base)
        .query_batch(&[query("openssl", "3.1.0")])
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::InvalidResponse { .. }),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn mismatched_result_count_is_an_error() {
    let app = osv_router(|_| BatchResponse { results: vec![] }, []);
    let base = spawn(app).await;

    let err = client(base)
        .query_batch(&[query("openssl", "3.1.0")])
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::UnexpectedResponse { .. }),
        "unexpected error: {err}"
    );
}
