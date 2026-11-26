//! Serde types for the [OSV](https://ossf.github.io/osv-schema/) vulnerability
//! database wire format: queries, batch responses, and vulnerability documents.
//!
//! Ecosystems are strings so OSV-compatible services can use names such as
//! `conda-forge`, which the upstream `osv` crate's closed enum cannot represent.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Treat nullable arrays from OSV producers like omitted fields.
fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

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
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub vulns: Vec<VulnRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResponse {
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub results: Vec<BatchResult>,
}

/// OSV vulnerability document (`GET /v1/vulns/{id}`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OsvVulnerability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// When set, the advisory has been withdrawn and should not be reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub severity: Vec<OsvSeverity>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub affected: Vec<OsvAffected>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub references: Vec<OsvReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_specific: Option<serde_json::Value>,
    /// Additional OSV fields preserved during serialization.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvSeverity {
    #[serde(rename = "type")]
    pub kind: String,
    pub score: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvAffected {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<OsvAffectedPackage>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub severity: Vec<OsvSeverity>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub ranges: Vec<OsvRange>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_specific: Option<serde_json::Value>,
    /// Additional OSV fields preserved during serialization.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvAffectedPackage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purl: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsvRange {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_specific: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
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
            ..Default::default()
        };
        let json = serde_json::to_value(&vuln).unwrap();
        assert_eq!(json, serde_json::json!({"id": "X"}));
    }

    #[test]
    fn nullable_osv_arrays_deserialize_as_empty() {
        let json = serde_json::json!({
            "id": "GHSA-g75f-g53v-794x",
            "aliases": null,
            "severity": null,
            "references": null,
            "affected": [{
                "package": {"ecosystem": "PyPI", "name": "bleach"},
                "severity": null,
                "ranges": null,
                "versions": null
            }]
        });

        let vuln: OsvVulnerability = serde_json::from_value(json).unwrap();
        assert!(vuln.aliases.is_empty());
        assert!(vuln.severity.is_empty());
        assert!(vuln.references.is_empty());
        assert!(vuln.affected[0].severity.is_empty());
        assert!(vuln.affected[0].ranges.is_empty());
        assert!(vuln.affected[0].versions.is_empty());
    }

    #[test]
    fn deserialize_basilisk_document_metadata_fields() {
        // Shape taken from basilisk's document builder: conda-forge affected
        // entries carry `versions` + `database_specific` (no ranges); the
        // document itself carries schema_version/published/withdrawn/details.
        let json = r#"{
            "schema_version": "1.6.0",
            "id": "CVE-2026-1234",
            "modified": "2026-01-01T00:00:00Z",
            "published": "2025-12-01T00:00:00Z",
            "withdrawn": "2026-02-01T00:00:00Z",
            "summary": "Buffer overflow",
            "details": "Long-form description.",
            "affected": [{
                "package": {
                    "ecosystem": "conda-forge",
                    "name": "pillow",
                    "purl": "pkg:conda/conda-forge/pillow"
                },
                "versions": ["10.1.0", "10.2.0"],
                "database_specific": {"basilisk": {"applicability": "evaluated"}}
            }],
            "database_specific": {"source": "nvd"}
        }"#;
        let vuln: OsvVulnerability = serde_json::from_str(json).unwrap();
        assert_eq!(vuln.schema_version.as_deref(), Some("1.6.0"));
        assert_eq!(vuln.published.as_deref(), Some("2025-12-01T00:00:00Z"));
        assert_eq!(vuln.withdrawn.as_deref(), Some("2026-02-01T00:00:00Z"));
        assert_eq!(vuln.details.as_deref(), Some("Long-form description."));
        assert!(vuln.database_specific.is_some());

        let affected = &vuln.affected[0];
        assert_eq!(
            affected.package.as_ref().unwrap().purl.as_deref(),
            Some("pkg:conda/conda-forge/pillow")
        );
        assert_eq!(
            affected.database_specific.as_ref().unwrap()["basilisk"]["applicability"],
            "evaluated"
        );

        let round_tripped = serde_json::to_value(&vuln).unwrap();
        assert_eq!(round_tripped["withdrawn"], "2026-02-01T00:00:00Z");
        assert_eq!(round_tripped["schema_version"], "1.6.0");
        assert_eq!(
            round_tripped["affected"][0]["database_specific"]["basilisk"]["applicability"],
            "evaluated"
        );
        assert_eq!(round_tripped["database_specific"]["source"], "nvd");
    }

    #[test]
    fn unmodeled_fields_round_trip_via_catch_all() {
        let json = serde_json::json!({
            "id": "CVE-2026-1",
            "related": ["CVE-2026-2"],
            "credits": [{"name": "someone", "type": "FINDER"}],
            "affected": [{
                "package": {"ecosystem": "PyPI", "name": "pillow"},
                "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
                "ecosystem_specific": {"functions": ["foo"]}
            }]
        });
        let vuln: OsvVulnerability = serde_json::from_value(json.clone()).unwrap();
        let round_tripped = serde_json::to_value(&vuln).unwrap();
        assert_eq!(round_tripped, json);
    }

    #[test]
    fn affected_severity_and_nested_range_fields_round_trip() {
        let json = serde_json::json!({
            "id": "CVE-2026-1234",
            "affected": [{
                "package": {"ecosystem": "PyPI", "name": "pillow"},
                "severity": [{
                    "type": "CVSS_V3",
                    "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
                    "source": "NVD"
                }],
                "ranges": [{
                    "type": "GIT",
                    "repo": "https://github.com/python-pillow/Pillow",
                    "database_specific": {"source": "upstream", "confidence": 0.9},
                    "events": [{"introduced": "0"}, {"limit": "deadbeef"}]
                }]
            }]
        });
        let doc: OsvVulnerability = serde_json::from_value(json.clone()).unwrap();
        let affected = &doc.affected[0];
        assert_eq!(affected.severity[0].kind, "CVSS_V3");
        assert!(!affected.extra.contains_key("severity"));
        let range = &affected.ranges[0];
        assert_eq!(
            range.repo.as_deref(),
            Some("https://github.com/python-pillow/Pillow")
        );
        assert_eq!(
            range.database_specific.as_ref().unwrap()["source"],
            "upstream"
        );
        assert_eq!(range.events[1].limit.as_deref(), Some("deadbeef"));
        assert_eq!(serde_json::to_value(doc).unwrap(), json);
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
