use super::*;
use crate::{
    test_support::{basilisk_client as client, osv_client, spawn_router as spawn},
    types::{BatchResponse, BatchResult, OsvQuery, QueryPackage},
};
use axum::{Router, routing::post};
use pixi_audit_test_support::osv_router;

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
async fn pypi_base_url_with_deep_path_keeps_all_segments() {
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
    let client = osv_client(deep);

    let results = client
        .osv_client()
        .query_batch(&[query("requests", "2.32.0")])
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].vulns.is_empty());
}

#[tokio::test]
async fn malformed_response_is_an_audit_error() {
    let app = Router::new().route("/v1/querybatch", post(|| async { "not JSON" }));
    let base = spawn(app).await;

    let err = client(base)
        .query_batch(&[query("openssl", "3.1.0")])
        .await
        .unwrap_err();
    assert!(
        matches!(err, AuditError::InvalidResponse { .. }),
        "unexpected error: {err}"
    );
}
