use std::collections::{BTreeSet, HashMap};

use futures::{StreamExt, TryStreamExt};
use serde::Serialize;

use crate::{
    client::{AuditError, BasiliskClient},
    types::{OsvQuery, OsvSeverity, OsvVulnerability, QueryPackage},
};

/// How many `GET /v1/vulns/{id}` requests to keep in flight.
const MAX_CONCURRENT_DETAIL_FETCHES: usize = 16;

/// Where a locked package comes from, for vulnerability-query purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageEcosystem {
    /// A conda package from the conda-forge channel. The only ecosystem
    /// basilisk's query API answers for today.
    CondaForge,
    /// A PyPI package.
    Pypi,
    /// A conda package from another channel (e.g. bioconda), or a source
    /// package. The contained string is a human-readable origin.
    Other(String),
}

impl PackageEcosystem {
    /// The ecosystem string sent in OSV queries.
    pub fn osv_ecosystem(&self) -> &str {
        match self {
            PackageEcosystem::CondaForge => "conda-forge",
            PackageEcosystem::Pypi => "PyPI",
            PackageEcosystem::Other(origin) => origin,
        }
    }

    /// Whether basilisk can currently answer for this ecosystem. Packages
    /// outside this set are reported as "not checked" (unless a query for
    /// them returns a finding anyway).
    pub fn is_audited(&self) -> bool {
        matches!(self, PackageEcosystem::CondaForge)
    }
}

/// A deduplicated locked package to audit.
#[derive(Debug, Clone)]
pub struct AuditPackage {
    pub name: String,
    pub version: String,
    pub ecosystem: PackageEcosystem,
    /// Environments (across all platforms) that contain this exact package.
    pub environments: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityBand {
    Unknown,
    Low,
    Medium,
    High,
    Critical,
}

impl SeverityBand {
    /// The highest CVSS band across the document's severity entries.
    /// Bands follow basilisk: critical >= 9.0, high >= 7.0, medium >= 4.0.
    pub fn from_severities(severities: &[OsvSeverity]) -> Self {
        severities
            .iter()
            .filter(|s| s.kind.starts_with("CVSS_V3"))
            .filter_map(|s| s.score.parse::<cvss::v3::Base>().ok())
            .map(|base| base.score().value())
            .map(|score| match score {
                s if s >= 9.0 => SeverityBand::Critical,
                s if s >= 7.0 => SeverityBand::High,
                s if s >= 4.0 => SeverityBand::Medium,
                _ => SeverityBand::Low,
            })
            .max()
            .unwrap_or(SeverityBand::Unknown)
    }
}

impl std::fmt::Display for SeverityBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            SeverityBand::Critical => "critical",
            SeverityBand::High => "high",
            SeverityBand::Medium => "medium",
            SeverityBand::Low => "low",
            SeverityBand::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

/// One (package, vulnerability) pair.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub package: String,
    pub version: String,
    pub ecosystem: String,
    pub environments: Vec<String>,
    pub id: String,
    pub aliases: Vec<String>,
    pub severity: SeverityBand,
    pub fixed_versions: Vec<String>,
    pub summary: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UncheckedPackage {
    pub package: String,
    pub version: String,
    pub ecosystem: String,
    pub environments: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditSummary {
    /// Packages in an ecosystem the database can answer for.
    pub audited: usize,
    pub vulnerable: usize,
    pub ignored: usize,
    pub unchecked: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditReport {
    pub vulnerabilities: Vec<Finding>,
    pub ignored: Vec<Finding>,
    pub unchecked: Vec<UncheckedPackage>,
    pub summary: AuditSummary,
}

fn build_query(package: &AuditPackage) -> OsvQuery {
    OsvQuery {
        package: QueryPackage {
            name: Some(package.name.to_lowercase()),
            ecosystem: Some(package.ecosystem.osv_ecosystem().to_string()),
            purl: None,
        },
        version: Some(package.version.clone()),
        page_token: None,
    }
}

/// Fixed versions for `package_name` from an OSV document: `fixed` events of
/// every `affected` entry that names the package (or names no package).
fn fixed_versions(doc: &OsvVulnerability, package_name: &str) -> Vec<String> {
    let mut versions = Vec::new();
    for affected in &doc.affected {
        let name_matches = affected
            .package
            .as_ref()
            .and_then(|p| p.name.as_deref())
            .is_none_or(|name| name.eq_ignore_ascii_case(package_name));
        if !name_matches {
            continue;
        }
        for range in &affected.ranges {
            for event in &range.events {
                if let Some(fixed) = &event.fixed
                    && !versions.contains(fixed)
                {
                    versions.push(fixed.clone());
                }
            }
        }
    }
    versions
}

fn advisory_url(doc: &OsvVulnerability) -> Option<String> {
    doc.references
        .iter()
        .find(|r| r.kind.as_deref() == Some("ADVISORY"))
        .or_else(|| doc.references.first())
        .map(|r| r.url.clone())
}

/// Audits `packages` against the vulnerability database, applying `ignore`
/// (vulnerability ids or aliases, case-insensitive).
pub async fn audit(
    client: &BasiliskClient,
    packages: Vec<AuditPackage>,
    ignore: &[String],
) -> Result<AuditReport, AuditError> {
    let queries: Vec<OsvQuery> = packages.iter().map(build_query).collect();
    let results = client.query_batch(&queries).await?;

    // Fetch each distinct vulnerability document once.
    let ids: BTreeSet<String> = results
        .iter()
        .flat_map(|r| r.vulns.iter().map(|v| v.id.clone()))
        .collect();
    let docs: HashMap<String, OsvVulnerability> = futures::stream::iter(ids)
        .map(|id| async move { client.get_vuln(&id).await.map(|doc| (id, doc)) })
        .buffer_unordered(MAX_CONCURRENT_DETAIL_FETCHES)
        .try_collect()
        .await?;

    let ignore_set: BTreeSet<String> = ignore.iter().map(|s| s.to_lowercase()).collect();

    let mut report = AuditReport::default();
    for (package, result) in packages.iter().zip(&results) {
        if package.ecosystem.is_audited() {
            report.summary.audited += 1;
        }
        if result.vulns.is_empty() {
            if !package.ecosystem.is_audited() {
                report.unchecked.push(UncheckedPackage {
                    package: package.name.clone(),
                    version: package.version.clone(),
                    ecosystem: package.ecosystem.osv_ecosystem().to_string(),
                    environments: package.environments.clone(),
                });
            }
            continue;
        }
        for vuln_ref in &result.vulns {
            let doc = docs
                .get(&vuln_ref.id)
                .expect("every referenced vulnerability was fetched");
            let finding = Finding {
                package: package.name.clone(),
                version: package.version.clone(),
                ecosystem: package.ecosystem.osv_ecosystem().to_string(),
                environments: package.environments.clone(),
                id: doc.id.clone(),
                aliases: doc.aliases.clone(),
                severity: SeverityBand::from_severities(&doc.severity),
                fixed_versions: fixed_versions(doc, &package.name),
                summary: doc.summary.clone(),
                url: advisory_url(doc),
            };
            let is_ignored = ignore_set.contains(&finding.id.to_lowercase())
                || finding
                    .aliases
                    .iter()
                    .any(|a| ignore_set.contains(&a.to_lowercase()));
            if is_ignored {
                report.ignored.push(finding);
            } else {
                report.vulnerabilities.push(finding);
            }
        }
    }

    // Highest severity first, then by package name, for stable output.
    report
        .vulnerabilities
        .sort_by(|a, b| b.severity.cmp(&a.severity).then(a.package.cmp(&b.package)));

    report.summary.vulnerable = report.vulnerabilities.len();
    report.summary.ignored = report.ignored.len();
    report.summary.unchecked = report.unchecked.len();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use url::Url;

    use super::*;
    use crate::client::BasiliskClient;

    #[test]
    fn severity_band_from_cvss_vector() {
        // CVSS:3.1 9.8 => critical
        assert_eq!(
            SeverityBand::from_severities(&[crate::types::OsvSeverity {
                kind: "CVSS_V3".to_string(),
                score: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H".to_string(),
            }]),
            SeverityBand::Critical
        );
        // Unparsable => unknown
        assert_eq!(SeverityBand::from_severities(&[]), SeverityBand::Unknown);
    }

    fn vuln_doc() -> serde_json::Value {
        serde_json::json!({
            "id": "BSLK-1",
            "aliases": ["CVE-2026-1234"],
            "summary": "Buffer overflow in openssl",
            "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
            "affected": [{
                "package": {"ecosystem": "conda-forge", "name": "openssl"},
                "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "3.1.1"}]}]
            }],
            "references": [{"type": "ADVISORY", "url": "https://example.com/adv"}]
        })
    }

    /// Mock server: openssl 3.1.0 has BSLK-1, everything else is clean.
    async fn spawn_mock() -> Url {
        let app = Router::new()
            .route(
                "/v1/querybatch",
                post(|Json(body): Json<serde_json::Value>| async move {
                    let results: Vec<serde_json::Value> = body["queries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|q| {
                            if q["package"]["name"] == "openssl" {
                                serde_json::json!({"vulns": [{"id": "BSLK-1"}]})
                            } else {
                                serde_json::json!({})
                            }
                        })
                        .collect();
                    Json(serde_json::json!({"results": results}))
                }),
            )
            .route("/v1/vulns/{id}", get(|| async { Json(vuln_doc()) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Url::parse(&format!("http://{addr}/")).unwrap()
    }

    fn test_client(base: Url) -> BasiliskClient {
        let http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        BasiliskClient::new(http, base)
    }

    fn packages() -> Vec<AuditPackage> {
        vec![
            AuditPackage {
                name: "openssl".to_string(),
                version: "3.1.0".to_string(),
                ecosystem: PackageEcosystem::CondaForge,
                environments: vec!["default".to_string()],
            },
            AuditPackage {
                name: "zlib".to_string(),
                version: "1.3".to_string(),
                ecosystem: PackageEcosystem::CondaForge,
                environments: vec!["default".to_string()],
            },
            AuditPackage {
                name: "requests".to_string(),
                version: "2.32.0".to_string(),
                ecosystem: PackageEcosystem::Pypi,
                environments: vec!["default".to_string()],
            },
        ]
    }

    #[tokio::test]
    async fn audit_reports_findings_and_unchecked() {
        let base = spawn_mock().await;
        let report = audit(&test_client(base), packages(), &[]).await.unwrap();

        assert_eq!(report.vulnerabilities.len(), 1);
        let finding = &report.vulnerabilities[0];
        assert_eq!(finding.package, "openssl");
        assert_eq!(finding.id, "BSLK-1");
        assert_eq!(finding.severity, SeverityBand::Critical);
        assert_eq!(finding.fixed_versions, vec!["3.1.1"]);
        assert_eq!(finding.url.as_deref(), Some("https://example.com/adv"));

        // The PyPI package is unchecked; the clean conda-forge package is not listed.
        assert_eq!(report.unchecked.len(), 1);
        assert_eq!(report.unchecked[0].package, "requests");

        assert_eq!(report.summary.audited, 2);
        assert_eq!(report.summary.vulnerable, 1);
        assert_eq!(report.summary.unchecked, 1);
        assert_eq!(report.summary.ignored, 0);
    }

    #[tokio::test]
    async fn ignore_list_matches_id_and_aliases_case_insensitively() {
        let base = spawn_mock().await;
        let report = audit(
            &test_client(base),
            packages(),
            &["cve-2026-1234".to_string()],
        )
        .await
        .unwrap();

        assert!(report.vulnerabilities.is_empty());
        assert_eq!(report.ignored.len(), 1);
        assert_eq!(report.ignored[0].id, "BSLK-1");
        assert_eq!(report.summary.ignored, 1);
    }

    #[tokio::test]
    async fn unchecked_package_with_finding_is_promoted() {
        // A PyPI-named "openssl" package would get the finding, not land in unchecked.
        let base = spawn_mock().await;
        let packages = vec![AuditPackage {
            name: "openssl".to_string(),
            version: "3.1.0".to_string(),
            ecosystem: PackageEcosystem::Pypi,
            environments: vec!["default".to_string()],
        }];
        let report = audit(&test_client(base), packages, &[]).await.unwrap();

        assert_eq!(report.vulnerabilities.len(), 1);
        assert!(report.unchecked.is_empty());
    }
}
