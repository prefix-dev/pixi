use serde::{Deserialize, Serialize};

/// Identity part of an OSV query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryPackage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purl: Option<String>,
}

/// A single query as accepted by `POST /v1/query` and `/v1/querybatch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OsvQuery {
    pub package: QueryPackage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

/// Reference to a vulnerability in a `querybatch` response (id + modified only).
#[derive(Debug, Clone, Deserialize)]
pub struct VulnRef {
    pub id: String,
    #[serde(default)]
    pub modified: Option<String>,
}

/// Per-query result in a `querybatch` response.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BatchResult {
    #[serde(default)]
    pub vulns: Vec<VulnRef>,
    #[serde(default)]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchResponse {
    #[serde(default)]
    pub results: Vec<BatchResult>,
}

/// Minimal OSV vulnerability document (`GET /v1/vulns/{id}`).
#[derive(Debug, Clone, Deserialize)]
pub struct OsvVulnerability {
    pub id: String,
    #[serde(default)]
    pub modified: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub severity: Vec<OsvSeverity>,
    #[serde(default)]
    pub affected: Vec<OsvAffected>,
    #[serde(default)]
    pub references: Vec<OsvReference>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsvSeverity {
    #[serde(rename = "type")]
    pub kind: String,
    pub score: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsvAffected {
    #[serde(default)]
    pub package: Option<OsvAffectedPackage>,
    #[serde(default)]
    pub ranges: Vec<OsvRange>,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsvAffectedPackage {
    #[serde(default)]
    pub ecosystem: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsvRange {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub events: Vec<OsvEvent>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OsvEvent {
    #[serde(default)]
    pub introduced: Option<String>,
    #[serde(default)]
    pub fixed: Option<String>,
    #[serde(default)]
    pub last_affected: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsvReference {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_query_skips_empty_fields() {
        let query = OsvQuery {
            package: QueryPackage {
                name: Some("openssl".to_string()),
                ecosystem: Some("conda-forge".to_string()),
                purl: None,
            },
            version: Some("3.1.0".to_string()),
            page_token: None,
        };
        let json = serde_json::to_value(&query).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "package": {"name": "openssl", "ecosystem": "conda-forge"},
                "version": "3.1.0"
            })
        );
    }

    #[test]
    fn deserialize_batch_response() {
        let json =
            r#"{"results":[{"vulns":[{"id":"BSLK-1","modified":"2026-01-01T00:00:00Z"}]},{}]}"#;
        let response: BatchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.results[0].vulns[0].id, "BSLK-1");
        assert!(response.results[1].vulns.is_empty());
        assert!(response.results[1].next_page_token.is_none());
    }

    #[test]
    fn deserialize_osv_document() {
        let json = r#"{
            "id": "BSLK-1",
            "modified": "2026-01-01T00:00:00Z",
            "aliases": ["CVE-2026-1234"],
            "summary": "Buffer overflow",
            "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
            "affected": [{
                "package": {"ecosystem": "conda-forge", "name": "openssl"},
                "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "3.1.1"}]}]
            }],
            "references": [{"type": "ADVISORY", "url": "https://example.com/adv"}]
        }"#;
        let vuln: OsvVulnerability = serde_json::from_str(json).unwrap();
        assert_eq!(vuln.id, "BSLK-1");
        assert_eq!(vuln.aliases, vec!["CVE-2026-1234"]);
        assert_eq!(
            vuln.affected[0].ranges[0].events[1].fixed.as_deref(),
            Some("3.1.1")
        );
        assert_eq!(vuln.references[0].url, "https://example.com/adv");
    }
}
