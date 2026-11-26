use axum::{Router, routing::get};
use url::Url;

use super::*;
use pixi_audit_test_support::{kev_router, spawn_router};

fn client(url: Url) -> KevClient {
    let http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
    KevClient::new(http, url)
}

fn feed() -> serde_json::Value {
    serde_json::json!({
        "title": "CISA Catalog of Known Exploited Vulnerabilities",
        "catalogVersion": "2026.09.01",
        "dateReleased": "2026-09-01T19:22:46.2162Z",
        "count": 2,
        "vulnerabilities": [
            {
                "cveID": "CVE-2026-82078",
                "vendorProject": "PaperCut",
                "product": "NG/MF",
                "vulnerabilityName": "PaperCut NG/MF Unsafe Reflection Vulnerability",
                "dateAdded": "2026-08-31",
                "shortDescription": "Unsafe reflection.",
                "requiredAction": "Apply mitigations per vendor instructions.",
                "dueDate": "2026-09-21",
                "knownRansomwareCampaignUse": "Known",
                "notes": "https://example.com/advisory",
                "cwes": ["CWE-470"]
            },
            {
                "cveID": "CVE-2022-3602",
                "vendorProject": "OpenSSL",
                "product": "OpenSSL",
                "vulnerabilityName": "OpenSSL X.509 buffer overflow",
                "dateAdded": "2022-11-01",
                "shortDescription": "Buffer overflow.",
                "requiredAction": "Apply updates per vendor instructions.",
                "dueDate": "2022-11-22",
                "knownRansomwareCampaignUse": "Unknown",
                "notes": ""
            }
        ]
    })
}

#[tokio::test]
async fn catalog_is_indexed_by_cve_case_insensitively() {
    let app = kev_router(feed());
    let url = spawn_router(app).await.join("kev.json").unwrap();

    let catalog = client(url).catalog().await.unwrap();

    assert_eq!(catalog.date_released(), Some("2026-09-01"));
    assert_eq!(catalog.len(), 2);
    let entry = catalog.get("cve-2026-82078").unwrap();
    assert_eq!(entry.cve, "CVE-2026-82078");
    assert_eq!(entry.vendor_project, "PaperCut");
    assert_eq!(entry.product, "NG/MF");
    assert_eq!(entry.date_added, "2026-08-31");
    assert_eq!(entry.due_date, "2026-09-21");
    assert_eq!(
        entry.known_ransomware_campaign_use,
        RansomwareCampaignUse::Known
    );
    assert_eq!(entry.notes, "https://example.com/advisory");
    assert_eq!(
        catalog
            .get("CVE-2022-3602")
            .unwrap()
            .known_ransomware_campaign_use,
        RansomwareCampaignUse::Unknown
    );
    assert!(catalog.get("CVE-2026-1").is_none());
}

#[tokio::test]
async fn entries_serialize_with_snake_case_fields() {
    let app = kev_router(feed());
    let url = spawn_router(app).await.join("kev.json").unwrap();
    let catalog = client(url).catalog().await.unwrap();

    let json = serde_json::to_value(catalog.get("CVE-2022-3602").unwrap()).unwrap();
    assert_eq!(json["cve"], "CVE-2022-3602");
    assert_eq!(json["known_ransomware_campaign_use"], "unknown");
    // Empty notes are left out.
    assert!(json.get("notes").is_none());
}

#[tokio::test]
async fn server_errors_are_reported() {
    let app = Router::new().route(
        "/kev.json",
        get(|| async { axum::http::StatusCode::NOT_FOUND }),
    );
    let url = spawn_router(app).await.join("kev.json").unwrap();
    let err = client(url).catalog().await.unwrap_err();
    assert!(matches!(
        err,
        ThreatFeedError::Status {
            status: reqwest::StatusCode::NOT_FOUND,
            ..
        }
    ));
}

#[tokio::test]
async fn malformed_feed_is_an_invalid_response() {
    let app = kev_router(serde_json::json!({"vulnerabilities": [{"product": "x"}]}));
    let url = spawn_router(app).await.join("kev.json").unwrap();
    let err = client(url).catalog().await.unwrap_err();
    assert!(
        matches!(err, ThreatFeedError::InvalidResponse { .. }),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn missing_vulnerabilities_is_an_invalid_response() {
    for body in [
        serde_json::json!({}),
        serde_json::json!({"message": "unrelated response"}),
    ] {
        let app = kev_router(body);
        let url = spawn_router(app).await.join("kev.json").unwrap();
        let err = client(url).catalog().await.unwrap_err();
        assert!(
            matches!(err, ThreatFeedError::InvalidResponse { .. }),
            "unexpected error: {err}"
        );
    }
}

#[tokio::test]
async fn explicit_empty_vulnerabilities_is_accepted() {
    let app = kev_router(serde_json::json!({"vulnerabilities": []}));
    let url = spawn_router(app).await.join("kev.json").unwrap();
    let catalog = client(url).catalog().await.unwrap();
    assert!(catalog.is_empty());
    assert_eq!(catalog.date_released(), None);
}
