use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::Query, extract::State, routing::get};
use url::Url;

use super::*;
use pixi_audit_test_support::{epss_router, spawn_router};

fn client(base_url: Url) -> EpssClient {
    let http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
    EpssClient::new(http, base_url)
}

/// Serves a score for every requested CVE except `CVE-2026-0000`, and
/// records the CVE count of each request.
async fn spawn_echo_mock(requests: Arc<Mutex<Vec<usize>>>) -> Url {
    let app = Router::new()
        .route(
            "/data/v1/epss",
            get(
                |State(requests): State<Arc<Mutex<Vec<usize>>>>,
                 Query(query): Query<HashMap<String, String>>| async move {
                    let cves: Vec<&str> = query["cve"].split(',').collect();
                    requests.lock().unwrap().push(cves.len());
                    let data: Vec<serde_json::Value> = cves
                        .iter()
                        .filter(|cve| **cve != "CVE-2026-0000")
                        .map(|cve| {
                            serde_json::json!({
                                "cve": cve,
                                "epss": "0.123450000",
                                "percentile": "0.900000000",
                                "date": "2026-09-01"
                            })
                        })
                        .collect();
                    Json(serde_json::json!({
                        "status": "OK",
                        "status-code": 200,
                        "total": data.len(),
                        "data": data
                    }))
                },
            ),
        )
        .with_state(requests);
    spawn_router(app).await
}

#[tokio::test]
async fn scores_are_requested_in_chunks_of_100_and_sorted() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let base = spawn_echo_mock(requests.clone()).await;
    let cves: Vec<String> = (0..150).rev().map(|n| format!("CVE-2026-{n:04}")).collect();

    let scores = client(base).scores(&cves).await.unwrap();

    assert_eq!(*requests.lock().unwrap(), vec![100, 50]);
    // CVE-2026-0000 is unscored and absent; the rest come back sorted.
    assert_eq!(scores.len(), 149);
    assert_eq!(scores[0].cve, "CVE-2026-0001");
    assert_eq!(scores[148].cve, "CVE-2026-0149");
    assert_eq!(scores[0].epss, 0.12345);
    assert_eq!(scores[0].percentile, 0.9);
    assert_eq!(scores[0].date, "2026-09-01");
}

#[tokio::test]
async fn no_cves_means_no_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let base = spawn_echo_mock(requests.clone()).await;
    let scores = client(base).scores(&[]).await.unwrap();
    assert!(scores.is_empty());
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn numeric_scores_are_accepted_too() {
    let app = epss_router(vec![serde_json::json!({
        "cve": "CVE-2026-1", "epss": 0.5, "percentile": 0.75, "date": "2026-09-01"
    })]);
    let scores = client(spawn_router(app).await)
        .scores(&["CVE-2026-1".to_string()])
        .await
        .unwrap();
    assert_eq!(scores[0].epss, 0.5);
    assert_eq!(scores[0].percentile, 0.75);
}

#[tokio::test]
async fn unparsable_scores_are_an_invalid_response() {
    let app = epss_router(vec![serde_json::json!({
        "cve": "CVE-2026-1", "epss": "high", "percentile": "0.75", "date": "2026-09-01"
    })]);
    let err = client(spawn_router(app).await)
        .scores(&["CVE-2026-1".to_string()])
        .await
        .unwrap_err();
    assert!(
        matches!(err, ThreatFeedError::InvalidResponse { .. }),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn missing_data_is_an_invalid_response() {
    for body in [
        serde_json::json!({}),
        serde_json::json!({"message": "unrelated response"}),
    ] {
        let app = Router::new()
            .route(
                "/data/v1/epss",
                get(|State(body): State<serde_json::Value>| async move { Json(body) }),
            )
            .with_state(body);
        let err = client(spawn_router(app).await)
            .scores(&["CVE-2026-1".to_string()])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ThreatFeedError::InvalidResponse { .. }),
            "unexpected error: {err}"
        );
    }
}

#[tokio::test]
async fn explicit_empty_data_is_accepted() {
    let app = epss_router(vec![]);
    let scores = client(spawn_router(app).await)
        .scores(&["CVE-2026-1".to_string()])
        .await
        .unwrap();
    assert!(scores.is_empty());
}

#[tokio::test]
async fn server_errors_are_reported() {
    let app = Router::new().route(
        "/data/v1/epss",
        get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
    );
    let err = client(spawn_router(app).await)
        .scores(&["CVE-2026-1".to_string()])
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ThreatFeedError::Status {
            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            ..
        }
    ));
}

#[tokio::test]
async fn base_url_with_deep_path_keeps_all_segments() {
    let app = Router::new().nest("/mirror", epss_router(vec![]));
    let base = spawn_router(app).await.join("mirror").unwrap();
    let scores = client(base)
        .scores(&["CVE-2026-1".to_string()])
        .await
        .unwrap();
    assert!(scores.is_empty());
}
