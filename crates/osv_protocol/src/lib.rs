//! Serde types for the [OSV](https://ossf.github.io/osv-schema/) vulnerability
//! database wire format: queries, batch responses, and vulnerability documents.
//!
//! This models the OSV API surface with an open `String` ecosystem instead of
//! the upstream [`osv`](https://crates.io/crates/osv) crate's closed enum, so
//! it can represent ecosystems that crate can't (e.g. `conda-forge`). Every
//! type derives both `Serialize` and `Deserialize` so the same definitions can
//! be shared between OSV clients (which serialize queries and deserialize
//! responses) and OSV-compatible servers (which do the reverse).

use serde::{Deserialize, Serialize};

/// Identity part of an OSV query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryPackage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purl: Option<String>,
}

/// A single query as accepted by `POST /v1/query` and `/v1/querybatch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsvQuery {
    pub package: QueryPackage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

/// Reference to a vulnerability in a `querybatch` response (id + modified only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VulnRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

/// Per-query result in a `querybatch` response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vulns: Vec<VulnRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<BatchResult>,
}

/// Minimal OSV vulnerability document (`GET /v1/vulns/{id}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvVulnerability {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub severity: Vec<OsvSeverity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected: Vec<OsvAffected>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<OsvReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvSeverity {
    #[serde(rename = "type")]
    pub kind: String,
    pub score: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvAffected {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<OsvAffectedPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<OsvRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvAffectedPackage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvRange {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<OsvEvent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OsvEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduced: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_affected: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvReference {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
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

    #[test]
    fn deserialize_query() {
        let json =
            r#"{"package": {"name": "openssl", "ecosystem": "conda-forge"}, "version": "3.1.0"}"#;
        let query: OsvQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.package.name.as_deref(), Some("openssl"));
        assert_eq!(query.package.ecosystem.as_deref(), Some("conda-forge"));
        assert_eq!(query.package.purl, None);
        assert_eq!(query.version.as_deref(), Some("3.1.0"));
        assert_eq!(query.page_token, None);
    }

    #[test]
    fn serialize_vulnerability_is_compact() {
        let vuln = OsvVulnerability {
            id: "X".to_string(),
            modified: None,
            aliases: Vec::new(),
            summary: None,
            severity: Vec::new(),
            affected: Vec::new(),
            references: Vec::new(),
        };
        let json = serde_json::to_value(&vuln).unwrap();
        assert_eq!(json, serde_json::json!({"id": "X"}));
    }

    #[test]
    fn vulnerability_round_trips() {
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
        let first: OsvVulnerability = serde_json::from_str(json).unwrap();
        let round_tripped = serde_json::to_string(&first).unwrap();
        let second: OsvVulnerability = serde_json::from_str(&round_tripped).unwrap();

        assert_eq!(second.id, "BSLK-1");
        assert_eq!(second.modified, first.modified);
        assert_eq!(second.aliases, first.aliases);
        assert_eq!(second.summary, first.summary);
        assert_eq!(second.severity[0].kind, "CVSS_V3");
        assert_eq!(
            second.affected[0].package.as_ref().unwrap().name.as_deref(),
            Some("openssl")
        );
        assert_eq!(
            second.affected[0].ranges[0].events[1].fixed.as_deref(),
            Some("3.1.1")
        );
        assert_eq!(second.references[0].url, "https://example.com/adv");
    }
}
