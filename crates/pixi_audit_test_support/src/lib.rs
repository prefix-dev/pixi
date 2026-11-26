use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    routing::{get, post},
};
use osv_protocol::{BatchResponse, OsvQuery, OsvVulnerability};
use serde_json::Value;
use url::Url;

pub async fn spawn_router(app: Router) -> Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Url::parse(&format!("http://{addr}/")).unwrap()
}

/// OSV batch queries with caller-defined matching and documents keyed by ID.
pub fn osv_router(
    query: impl Fn(Vec<OsvQuery>) -> BatchResponse + Send + Sync + 'static,
    documents: impl IntoIterator<Item = OsvVulnerability>,
) -> Router {
    let query = Arc::new(query);
    let documents: HashMap<_, _> = documents
        .into_iter()
        .map(|doc| (doc.id.clone(), doc))
        .collect();
    let documents = Arc::new(documents);
    Router::new()
        .route(
            "/v1/querybatch",
            post(move |Json(body): Json<Value>| {
                let query = Arc::clone(&query);
                async move {
                    let queries = serde_json::from_value(body["queries"].clone()).unwrap();
                    Json(query(queries))
                }
            }),
        )
        .route(
            "/v1/vulns/{id}",
            get(move |Path(id): Path<String>| {
                let documents = Arc::clone(&documents);
                async move {
                    documents
                        .get(&id)
                        .cloned()
                        .map(Json)
                        .ok_or(StatusCode::NOT_FOUND)
                }
            }),
        )
}

/// FIRST-shaped response, filtered to the CVEs requested by the client.
pub fn epss_router(rows: Vec<Value>) -> Router {
    let rows = Arc::new(rows);
    Router::new().route(
        "/data/v1/epss",
        get(move |Query(params): Query<HashMap<String, String>>| {
            let rows = Arc::clone(&rows);
            async move {
                let requested = params.get("cve").map(String::as_str).unwrap_or("");
                let data: Vec<_> = rows
                    .iter()
                    .filter(|row| {
                        row["cve"].as_str().is_some_and(|cve| {
                            requested.split(',').any(|id| id.eq_ignore_ascii_case(cve))
                        })
                    })
                    .cloned()
                    .collect();
                Json(serde_json::json!({"status": "OK", "data": data}))
            }
        }),
    )
}

pub fn kev_router(catalog: Value) -> Router {
    Router::new().route(
        "/kev.json",
        get(move || {
            let catalog = catalog.clone();
            async move { Json(catalog) }
        }),
    )
}
