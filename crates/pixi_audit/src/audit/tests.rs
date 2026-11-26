use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    routing::{get, post},
};
use pixi_audit_test_support::{epss_router, kev_router, osv_router};
use threat_feeds::RansomwareCampaignUse;
use url::Url;

use super::*;
use crate::{
    CondaForgeBuild, IgnoreScope, MatchConfidence, MatchedIdentity, MatchedIdentityKind,
    OtherEcosystemFix, SeverityRating, UpstreamRelease,
};
use crate::{
    client::BasiliskClient,
    test_support::{
        basilisk_client as test_client, conda_name, osv_client as test_osv_client, pypi_name,
        spawn_router as spawn,
    },
};

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
    spawn_mock_serving(vuln_doc()).await
}

async fn audit(
    client: &BasiliskClient,
    packages: Vec<AuditPackage>,
    ignore: &[IgnoreRule],
) -> Result<AuditReport, AuditError> {
    let osv_client = clean_osv_client().await;
    super::audit(
        client,
        &osv_client,
        packages,
        ignore,
        ThreatSources::default(),
    )
    .await
}

/// A PyPI database that reports every package as clean.
async fn clean_osv_client() -> OsvDevClient {
    let app = osv_router(
        |queries| osv_protocol::BatchResponse {
            results: vec![BatchResult::default(); queries.len()],
        },
        [],
    );
    test_osv_client(spawn(app).await)
}

fn packages() -> Vec<AuditPackage> {
    vec![
        AuditPackage {
            name: conda_name("openssl"),
            version: Some("3.1.0".to_string()),
            ecosystem: PackageEcosystem::CondaForge,
            environments: vec!["default".to_string()],
        },
        AuditPackage {
            name: conda_name("zlib"),
            version: Some("1.3".to_string()),
            ecosystem: PackageEcosystem::CondaForge,
            environments: vec!["default".to_string()],
        },
        AuditPackage {
            name: pypi_name("requests"),
            version: Some("2.32.0".to_string()),
            ecosystem: PackageEcosystem::Pypi,
            environments: vec!["default".to_string()],
        },
    ]
}

/// Like `vuln_doc`, but with no fixed events anywhere (id BSLK-NOFIX).
fn vuln_doc_nofix() -> serde_json::Value {
    serde_json::json!({
        "id": "BSLK-NOFIX",
        "aliases": ["CVE-2026-7777"],
        "summary": "No fix yet",
        "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}],
        "affected": [{
            "package": {"ecosystem": "conda-forge", "name": "openssl"},
            "versions": ["3.1.0"]
        }]
    })
}

fn vuln_doc_with_git_fix() -> serde_json::Value {
    serde_json::json!({
        "id": "BSLK-GIT-FIX",
        "summary": "Fixed in source but not released",
        "affected": [{
            "package": {"ecosystem": "conda-forge", "name": "openssl"},
            "ranges": [{
                "type": "GIT",
                "events": [{"introduced": "0"}, {"fixed": "deadbeef"}]
            }]
        }]
    })
}

fn vuln_doc_with_basilisk_upstream_release() -> serde_json::Value {
    serde_json::json!({
        "id": "CVE-2026-50219",
        "summary": "Known upstream release",
        "affected": [{
            "package": {"ecosystem": "conda-forge", "name": "openssl"},
            "versions": ["3.1.0"],
            "database_specific": {"basilisk": {
                "fix_version": "3.1.1",
                "conda_publication": "not_published"
            }}
        }]
    })
}

/// Mock server serving `doc` for every openssl query.
async fn spawn_mock_serving(doc: serde_json::Value) -> Url {
    let doc: OsvVulnerability = serde_json::from_value(doc).unwrap();
    let vuln_id = doc.id.clone();
    let app = osv_router(
        move |queries| {
            serde_json::from_value(serde_json::json!({
                "results": queries.iter().map(|query| {
                    if query.package.name.as_deref() == Some("openssl") {
                        serde_json::json!({"vulns": [{"id": vuln_id}]})
                    } else {
                        serde_json::json!({})
                    }
                }).collect::<Vec<_>>()
            }))
            .unwrap()
        },
        [doc],
    );
    spawn(app).await
}

fn assemble_fixture(
    doc: serde_json::Value,
    packages: Vec<AuditPackage>,
    ignore: &[IgnoreRule],
) -> Result<AuditReport, AuditError> {
    let doc: OsvVulnerability = serde_json::from_value(doc).unwrap();
    let planned: Vec<_> = packages.iter().filter_map(plan_query).collect();
    let results = planned
        .iter()
        .map(|query| {
            if query.package.name.as_str() == "openssl" {
                serde_json::from_value(serde_json::json!({"vulns": [{"id": doc.id}]})).unwrap()
            } else {
                BatchResult::default()
            }
        })
        .collect::<Vec<_>>();
    let documents = HashMap::from([((Database::Basilisk, doc.id.clone()), doc)]);
    assemble_report(&planned, &results, &documents, ignore)
}

fn ignore_until_fixed(rules: &[IgnoreRule]) -> &[IgnoreRule] {
    rules
}

#[test]
fn until_fixed_with_no_known_fix_is_ignored_with_reason() {
    let doc = vuln_doc_nofix();
    let ids = vec![IgnoreRule::from_id(
        "BSLK-NOFIX".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();
    assert!(report.matches.is_empty());
    assert_eq!(report.ignored.len(), 1);
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::UntilFixed));
    // The JSON field serializes kebab-case.
    let json = serde_json::to_value(&report.ignored[0]).unwrap();
    assert_eq!(json["ignore_mode"], "until-fixed");
}

#[test]
fn git_fix_does_not_disable_until_fixed_ignore() {
    let doc = vuln_doc_with_git_fix();
    let ids = vec![IgnoreRule::from_id(
        "BSLK-GIT-FIX".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.ignored.len(), 1);
    assert!(report.ignored[0].fixed_versions.is_empty());
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::UntilFixed));
}

#[test]
fn upstream_only_fix_does_not_end_until_fixed_ignore() {
    let doc = vuln_doc_with_basilisk_upstream_release();
    let ids = vec![IgnoreRule::from_id(
        "CVE-2026-50219".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::UntilFixed));
    assert_eq!(
        report.ignored[0].upstream_releases,
        vec![UpstreamRelease {
            version: "3.1.1".to_string(),
            conda_forge_build: CondaForgeBuild::Missing,
        }]
    );
}

#[test]
fn fix_of_an_older_branch_does_not_end_until_fixed_ignore() {
    let doc = serde_json::json!({
        "id": "BSLK-BRANCH",
        "affected": [{
            "package": {"ecosystem": "conda-forge", "name": "openssl"},
            "ranges": [{"type": "ECOSYSTEM", "events": [
                {"introduced": "0"}, {"fixed": "3.0.9"}, {"introduced": "3.1.0"}
            ]}]
        }]
    });
    let ids = vec![IgnoreRule::from_id(
        "BSLK-BRANCH".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();
    assert!(report.matches.is_empty());
    assert_eq!(report.ignored[0].fixed_versions, ["3.0.9"]);
}

#[test]
fn other_ecosystem_fix_does_not_end_until_fixed_ignore() {
    let mut doc = vuln_doc_nofix();
    doc["affected"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "package": {"ecosystem": "PyPI", "name": "openssl"},
            "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "3.2.0"}]}]
        }));
    let ids = vec![IgnoreRule::from_id(
        "BSLK-NOFIX".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.ignored[0].other_ecosystem_fixes.len(), 1);
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::UntilFixed));
}

fn package_rule(package: &str, id: Option<&str>, mode: IgnoreMode) -> IgnoreRule {
    let scope = IgnoreScope {
        package: Some(package.to_string()),
        ..IgnoreScope::default()
    };
    IgnoreRule::new(id.map(str::to_string), scope, mode).unwrap()
}

/// Whether `rule` ignores `vuln_match`, parsing the locked version the way
/// the audit does.
fn applies(rule: &IgnoreRule, vuln_match: &Match) -> bool {
    let locked = vuln_match.package.parsed_version().ok();
    rule.applies_to(vuln_match, locked.as_ref())
}

#[test]
fn package_rules_scope_ignores_to_one_package() {
    // BSLK-1 (alias CVE-2026-1234) affects openssl; nothing else is found.
    let cases: [(IgnoreRule, Option<IgnoreMode>); 5] = [
        // Every vulnerability match in the package, matched case-insensitively.
        (
            package_rule("OpenSSL", None, IgnoreMode::Permanent),
            Some(IgnoreMode::Permanent),
        ),
        // Another package: no effect.
        (package_rule("zlib", None, IgnoreMode::Permanent), None),
        // One id in the package, by alias.
        (
            package_rule("openssl", Some("cve-2026-1234"), IgnoreMode::Permanent),
            Some(IgnoreMode::Permanent),
        ),
        // A different id in the package: no effect.
        (
            package_rule("openssl", Some("CVE-9999-1"), IgnoreMode::Permanent),
            None,
        ),
        // Until-fixed by id lapses because BSLK-1 has a fix.
        (
            package_rule("openssl", Some("BSLK-1"), IgnoreMode::UntilFixed),
            None,
        ),
    ];
    for (rule, expected) in cases {
        let doc = vuln_doc();
        let rules = [rule.clone()];
        let spec = &rules;
        let report = assemble_fixture(doc, packages(), spec).unwrap();
        let actual = report
            .ignored
            .first()
            .and_then(|vuln_match| vuln_match.ignore_mode);
        assert_eq!(actual, expected, "rule: {rule:?}");
        assert_eq!(
            report.matches.len() + report.ignored.len(),
            1,
            "rule: {rule:?}"
        );
    }
}

#[test]
fn scoped_id_rule_only_ignores_the_selected_ecosystem_and_version() {
    let doc = vuln_doc();
    let report = assemble_fixture(doc, packages(), &[]).unwrap();
    let mut conda = report.matches[0].clone();
    conda.package.name = conda_name("openssl");

    // A version alone names no package, so it would ignore everything.
    assert_eq!(
        IgnoreRule::new(
            None,
            IgnoreScope {
                version: Some(conda.package.version.clone()),
                ..IgnoreScope::default()
            },
            IgnoreMode::Permanent,
        ),
        Err(crate::InvalidIgnoreRule::MissingTarget)
    );

    let mut pypi = conda.clone();
    pypi.package.name = pypi_name("openssl");
    pypi.package.ecosystem = PackageEcosystem::Pypi;

    let rule = IgnoreRule::new(
        Some("CVE-2026-1234".to_string()),
        IgnoreScope {
            ecosystem: Some("conda-forge".to_string()),
            version: Some("3.1.0".to_string()),
            ..IgnoreScope::default()
        },
        IgnoreMode::Permanent,
    )
    .unwrap();
    assert!(applies(&rule, &conda));
    assert!(!applies(&rule, &pypi));
    conda.package.version = "3.1.1".to_string();
    assert!(!applies(&rule, &conda));

    let rule = IgnoreRule::new(
        None,
        IgnoreScope {
            package: Some("openssl".to_string()),
            up_to_version: Some("3.1.0".to_string()),
            ..IgnoreScope::default()
        },
        IgnoreMode::Permanent,
    )
    .unwrap();
    assert!(!applies(&rule, &conda));
    conda.package.version = "3.1.0".to_string();
    assert!(applies(&rule, &conda));
    conda.package.version = "3.0.9".to_string();
    assert!(applies(&rule, &conda));
    let doc = vuln_doc();
    let stale = [IgnoreRule::new(
        Some("CVE-2026-1234".to_string()),
        IgnoreScope {
            ecosystem: Some("conda-forge".to_string()),
            version: Some("3.1.1".to_string()),
            ..IgnoreScope::default()
        },
        IgnoreMode::Permanent,
    )
    .unwrap()];
    let report = assemble_fixture(doc, packages(), &stale).unwrap();
    assert_eq!(report.summary.vulnerable, 1);
}

#[test]
fn until_fixed_rules_need_an_advisory_id() {
    let scope = IgnoreScope {
        package: Some("openssl".to_string()),
        ..IgnoreScope::default()
    };
    assert_eq!(
        IgnoreRule::new(None, scope, IgnoreMode::UntilFixed),
        Err(crate::InvalidIgnoreRule::UntilFixedWithoutId)
    );
    assert_eq!(
        IgnoreRule::new(None, IgnoreScope::default(), IgnoreMode::Permanent),
        Err(crate::InvalidIgnoreRule::MissingTarget)
    );
}

#[test]
fn ignore_rules_that_match_no_match_are_reported() {
    // BSLK-1 (alias CVE-2026-1234) is the only vulnerability match. Matching is by id
    // or alias, case-insensitive, and an until-fixed rule that matched a
    // vulnerability match with a known fix still counts as matched.
    let doc = vuln_doc();
    let rules = [
        IgnoreRule::from_id("cve-2026-1234".to_string(), IgnoreMode::Permanent),
        IgnoreRule::from_id("BSLK-1".to_string(), IgnoreMode::Permanent),
        IgnoreRule::from_id("CVE-9999-0001".to_string(), IgnoreMode::Permanent),
        IgnoreRule::from_id("cve-9999-0001".to_string(), IgnoreMode::Permanent),
        IgnoreRule::from_id("bslk-1".to_string(), IgnoreMode::UntilFixed),
        IgnoreRule::from_id("cve-9999-0001".to_string(), IgnoreMode::UntilFixed),
        IgnoreRule::from_id("GHSA-nope".to_string(), IgnoreMode::UntilFixed),
    ];
    let spec = &rules;
    let report = assemble_fixture(doc, packages(), spec).unwrap();

    assert_eq!(report.ignored.len(), 1);
    assert_eq!(report.unused_ignores, ["CVE-9999-0001", "GHSA-nope"]);

    // Nothing to ignore, no unused ignores.
    let doc = vuln_doc();
    let report = assemble_fixture(doc, packages(), &[]).unwrap();
    assert!(report.unused_ignores.is_empty());
}

#[test]
fn unbounded_rules_match_even_when_the_locked_version_is_invalid() {
    let doc = vuln_doc();
    let report = assemble_fixture(doc, packages(), &[]).unwrap();
    let mut vuln_match = report.matches[0].clone();
    vuln_match.package.version = "not a version".to_string();
    let bounded = IgnoreRule::new(
        Some("BSLK-1".to_string()),
        IgnoreScope {
            up_to_version: Some("3.1.0".to_string()),
            ..IgnoreScope::default()
        },
        IgnoreMode::Permanent,
    )
    .unwrap();
    let rules = [
        IgnoreRule::from_id("cve-2026-1234".to_string(), IgnoreMode::Permanent),
        package_rule("openssl", None, IgnoreMode::Permanent),
        bounded,
    ];
    let mut ignores = Ignores::new(&rules);
    assert_eq!(ignores.reason_for(&vuln_match), Some(IgnoreMode::Permanent));
}

#[test]
fn until_fixed_with_known_fix_stays_vulnerable() {
    // BSLK-1 has fixed 3.1.1, so ignore does not apply.
    let doc = vuln_doc();
    let ids = vec![IgnoreRule::from_id(
        "BSLK-1".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();
    assert_eq!(report.matches.len(), 1);
    assert!(report.ignored.is_empty());
    assert_eq!(report.matches[0].ignore_mode, None);
    assert!(report.unused_ignores.is_empty());
}

#[test]
fn until_fixed_matches_aliases_case_insensitively() {
    let doc = vuln_doc_nofix();
    let ids = vec![IgnoreRule::from_id(
        "cve-2026-7777".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = assemble_fixture(doc, packages(), ignore_until_fixed(&ids)).unwrap();
    assert_eq!(report.ignored.len(), 1);
}

#[test]
fn ignore_wins_over_until_fixed() {
    // BSLK-1 has a fix; plain ignore still ignores it, reason Always.
    let doc = vuln_doc();
    let rules = [
        IgnoreRule::from_id("BSLK-1".to_string(), IgnoreMode::UntilFixed),
        IgnoreRule::from_id("BSLK-1".to_string(), IgnoreMode::Permanent),
    ];
    let spec = &rules;
    let report = assemble_fixture(doc, packages(), spec).unwrap();
    assert!(report.matches.is_empty());
    assert_eq!(report.ignored.len(), 1);
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::Permanent));
    assert!(report.unused_ignores.is_empty());
}

#[test]
fn plain_ignored_match_has_permanent_mode_and_it_serializes() {
    let rules = [IgnoreRule::from_id(
        "BSLK-1".to_string(),
        IgnoreMode::Permanent,
    )];
    let spec = &rules;
    let report = assemble_fixture(vuln_doc(), packages(), spec).unwrap();
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::Permanent));
    let json = serde_json::to_value(&report.ignored[0]).unwrap();
    assert_eq!(json["ignore_mode"], "permanent");
    // Active vulnerability matches must NOT carry the key at all.
    let clean = assemble_fixture(vuln_doc(), packages(), &[]).unwrap();
    let vuln_json = serde_json::to_value(&clean.matches[0]).unwrap();
    assert!(vuln_json.get("ignore_mode").is_none());
}

#[test]
fn affected_package_severity_is_scored_without_top_level_severity() {
    let mut doc = vuln_doc();
    doc["affected"][0]["severity"] = doc.as_object_mut().unwrap().remove("severity").unwrap();
    let report = assemble_fixture(doc, packages(), &[]).unwrap();
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].severity, SeverityRating::Critical);
    assert_eq!(report.matches[0].cvss_base_score, Some(9.8));
}

#[test]
fn unrelated_affected_packages_do_not_contribute_severity() {
    let mut doc = vuln_doc();
    let critical = doc.as_object_mut().unwrap().remove("severity").unwrap();
    doc["affected"][0]["severity"] = serde_json::json!([{
        "type": "CVSS_V3",
        "score": "CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:U/C:H/I:N/A:N"
    }]);
    doc["affected"].as_array_mut().unwrap().extend([
        serde_json::json!({
            "package": {"ecosystem": "conda-forge", "name": "unrelated"},
            "severity": critical
        }),
        serde_json::json!({
            "package": {"ecosystem": "PyPI", "name": "unrelated"},
            "severity": critical
        }),
    ]);
    let report = assemble_fixture(doc, packages(), &[]).unwrap();
    assert_eq!(report.matches[0].severity, SeverityRating::Medium);
    assert_eq!(report.matches[0].cvss_base_score, Some(5.3));
}

#[test]
fn top_level_and_matched_package_severity_are_combined() {
    let mut doc = vuln_doc();
    doc["affected"][0]["severity"] = serde_json::json!([{
        "type": "CVSS_V3",
        "score": "CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:U/C:H/I:N/A:N"
    }]);
    let report = assemble_fixture(doc, packages(), &[]).unwrap();
    assert_eq!(report.matches[0].severity, SeverityRating::Critical);
    assert!((report.matches[0].cvss_base_score.unwrap() - 7.55).abs() < 1e-9);
}

#[tokio::test]
async fn non_ascii_advisory_identifiers_do_not_break_cve_extraction() {
    let mut doc = vuln_doc();
    doc["id"] = serde_json::json!("ABCé-1");
    doc["aliases"] = serde_json::json!(["ABCé-2", "cVe-2026-1234"]);
    let base = spawn_mock_serving(doc).await;
    let report = audit(&test_client(base), packages(), &[]).await.unwrap();
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].id, "ABCé-1");
    assert_eq!(report.matches[0].cve_ids(), ["CVE-2026-1234"]);
}

#[tokio::test]
async fn audit_reports_match_and_not_audited() {
    let base = spawn_mock().await;
    let report = audit(&test_client(base), packages(), &[]).await.unwrap();

    assert_eq!(report.matches.len(), 1);
    let vuln_match = &report.matches[0];
    assert_eq!(vuln_match.package.name.as_str(), "openssl");
    assert_eq!(vuln_match.id, "BSLK-1");
    assert_eq!(vuln_match.severity, SeverityRating::Critical);
    assert_eq!(vuln_match.fixed_versions, vec!["3.1.1"]);
    assert_eq!(
        vuln_match.url.as_deref(),
        Some("https://basilisk.prefix.dev/advisories/BSLK-1")
    );

    // PyPI and clean conda-forge packages are audited and not listed as not audited.
    assert!(report.not_audited.is_empty());

    assert_eq!(report.summary.audited, 3);
    assert_eq!(report.summary.vulnerable, 1);
    assert_eq!(report.summary.not_audited, 0);
    assert_eq!(report.summary.ignored, 0);
}

#[test]
fn ignored_match_are_sorted_like_vulnerabilities() {
    let packages: Vec<_> = packages()
        .into_iter()
        .filter(|package| package.ecosystem == PackageEcosystem::CondaForge)
        .collect();
    let planned: Vec<_> = packages.iter().filter_map(plan_query).collect();
    let result: BatchResult =
        serde_json::from_value(serde_json::json!({"vulns": [{"id": "BSLK-1"}]})).unwrap();
    let results = vec![result; planned.len()];
    let documents = HashMap::from([(
        (Database::Basilisk, "BSLK-1".to_string()),
        serde_json::from_value(vuln_doc()).unwrap(),
    )]);

    let rules = [IgnoreRule::from_id(
        "BSLK-1".to_string(),
        IgnoreMode::Permanent,
    )];
    let spec = &rules;
    let mut report = assemble_report(&planned, &results, &documents, spec).unwrap();
    report.sort(SortKey::default());

    // Same severity everywhere, so the tie-break is the package name.
    let order: Vec<&str> = report
        .ignored
        .iter()
        .map(|f| f.package.name.as_str())
        .collect();
    assert_eq!(order, vec!["openssl", "zlib"]);
}

#[test]
fn ignore_list_matches_id_and_aliases_case_insensitively() {
    let doc = vuln_doc();
    let rules = [IgnoreRule::from_id(
        "cve-2026-1234".to_string(),
        IgnoreMode::Permanent,
    )];
    let spec = &rules;
    let report = assemble_fixture(doc, packages(), spec).unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.ignored.len(), 1);
    assert_eq!(report.ignored[0].id, "BSLK-1");
    assert_eq!(report.summary.ignored, 1);
}

/// Mock server where the only advisory has been withdrawn.
async fn spawn_withdrawn_mock() -> Url {
    let mut doc = vuln_doc();
    doc["withdrawn"] = serde_json::json!("2026-02-01T00:00:00Z");
    spawn_mock_serving(doc).await
}

#[tokio::test]
async fn unknown_version_packages_are_not_audited_and_never_queried() {
    // A version-less OSV query matches ALL versions of a package, which
    // would flood the report with false positives — so a package whose
    // version could not be determined must not be queried at all.
    let received: Arc<Mutex<usize>> = Arc::default();
    let received_clone = received.clone();
    let app = Router::new()
        .route(
            "/v1/querybatch",
            post(
                move |axum::extract::State(received): axum::extract::State<Arc<Mutex<usize>>>,
                      Json(body): Json<serde_json::Value>| async move {
                    let n = body["queries"].as_array().unwrap().len();
                    *received.lock().unwrap() += n;
                    let results: Vec<serde_json::Value> =
                        (0..n).map(|_| serde_json::json!({})).collect();
                    Json(serde_json::json!({"results": results}))
                },
            ),
        )
        .with_state(received_clone);
    let base = spawn(app).await;

    let packages = vec![
        AuditPackage {
            name: conda_name("mystery"),
            version: None,
            ecosystem: PackageEcosystem::CondaForge,
            environments: vec!["default".to_string()],
        },
        AuditPackage {
            name: conda_name("zlib"),
            version: Some("1.3".to_string()),
            ecosystem: PackageEcosystem::CondaForge,
            environments: vec!["default".to_string()],
        },
    ];
    let report = audit(&test_client(base), packages, &[]).await.unwrap();

    // Only zlib was queried; mystery is not audited and not counted audited.
    assert_eq!(*received.lock().unwrap(), 1);
    assert!(report.matches.is_empty());
    assert_eq!(report.not_audited.len(), 1);
    assert_eq!(report.not_audited[0].name.as_str(), "mystery");
    assert_eq!(report.not_audited[0].version, None);
    assert_eq!(report.summary.audited, 1);
}

#[tokio::test]
async fn other_ecosystem_packages_are_not_audited_and_never_queried() {
    // No database answers for bioconda or source packages. Querying Basilisk
    // for them anyway could only ever produce vulnerability matches that are counted
    // neither as audited nor as not audited, so they are not sent at all.
    let received: Arc<Mutex<usize>> = Arc::default();
    let received_clone = received.clone();
    let app = Router::new()
        .route(
            "/v1/querybatch",
            post(
                move |axum::extract::State(received): axum::extract::State<Arc<Mutex<usize>>>,
                      Json(body): Json<serde_json::Value>| async move {
                    let n = body["queries"].as_array().unwrap().len();
                    *received.lock().unwrap() += n;
                    let results: Vec<serde_json::Value> =
                        (0..n).map(|_| serde_json::json!({})).collect();
                    Json(serde_json::json!({"results": results}))
                },
            ),
        )
        .with_state(received_clone);
    let base = spawn(app).await;

    let packages = vec![
        AuditPackage {
            name: conda_name("samtools"),
            version: Some("1.19".to_string()),
            ecosystem: PackageEcosystem::Other("bioconda".to_string()),
            environments: vec!["default".to_string()],
        },
        AuditPackage {
            name: conda_name("zlib"),
            version: Some("1.3".to_string()),
            ecosystem: PackageEcosystem::CondaForge,
            environments: vec!["default".to_string()],
        },
    ];
    let report = audit(&test_client(base), packages, &[]).await.unwrap();

    assert_eq!(*received.lock().unwrap(), 1);
    assert_eq!(report.not_audited.len(), 1);
    assert_eq!(report.not_audited[0].name.as_str(), "samtools");
    assert_eq!(
        report.not_audited[0].ecosystem,
        PackageEcosystem::Other("bioconda".to_string())
    );
    assert_eq!(report.summary.audited, 1);
    assert_eq!(report.summary.not_audited, 1);
}

/// Mock server whose detail endpoint fails with `status` for every id.
async fn spawn_broken_details_mock(status: axum::http::StatusCode) -> Url {
    let app = Router::new()
        .route(
            "/v1/querybatch",
            post(|Json(body): Json<serde_json::Value>| async move {
                let n = body["queries"].as_array().unwrap().len();
                let results: Vec<serde_json::Value> = (0..n)
                    .map(|_| serde_json::json!({"vulns": [{"id": "BSLK-GONE"}]}))
                    .collect();
                Json(serde_json::json!({"results": results}))
            }),
        )
        .route("/v1/vulns/{id}", get(move || async move { status }));
    spawn(app).await
}

#[tokio::test]
async fn missing_detail_document_fails_the_audit_for_each_database() {
    let base = spawn_broken_details_mock(axum::http::StatusCode::NOT_FOUND).await;
    for ecosystem in [PackageEcosystem::CondaForge, PackageEcosystem::Pypi] {
        let package = AuditPackage {
            name: match ecosystem {
                PackageEcosystem::Pypi => pypi_name("openssl"),
                _ => conda_name("openssl"),
            },
            version: Some("3.1.0".to_string()),
            ecosystem,
            environments: vec!["default".to_string()],
        };
        let err = super::audit(
            &test_client(base.clone()),
            &test_osv_client(base.clone()),
            vec![package],
            &[],
            ThreatSources::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("404"), "unexpected error: {err}");
    }
}

#[tokio::test]
async fn server_detail_fetch_errors_fail_the_audit() {
    let base = spawn_broken_details_mock(axum::http::StatusCode::INTERNAL_SERVER_ERROR).await;
    let err = audit(&test_client(base), packages(), &[])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("500"), "unexpected error: {err}");
}

#[tokio::test]
async fn withdrawn_vulnerabilities_are_not_reported() {
    let base = spawn_withdrawn_mock().await;
    let report = audit(&test_client(base), packages(), &[]).await.unwrap();

    assert!(report.matches.is_empty());
    assert!(report.ignored.is_empty());
    assert_eq!(report.summary.vulnerable, 0);
    // PyPI is audited independently through OSV, so a clean package is
    // audited rather than listed as not audited.
    assert!(report.not_audited.is_empty());
}

fn pypi_doc() -> OsvVulnerability {
    serde_json::from_value(serde_json::json!({
                    "id": "PYSEC-2026-1",
                    "modified": "2026-07-31T00:00:00Z",
                    "summary": "Python package\nvulnerability",
                    "aliases": ["CVE-2026-9999"],
                    "severity": [{
                        "type": "CVSS_V3",
                        "score": "CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:U/C:H/I:N/A:N"
                    }],
                    "affected": [{
                        "package": {"ecosystem": "PyPI", "name": "requests-test"},
                        "ranges": [{
                            "type": "ECOSYSTEM",
                            "events": [{"introduced": "0"}, {"fixed": "2.32.1"}]
                        }]
                    }],
                    "references": [{
                        "type": "ADVISORY",
                        "url": "https://example.com/pypi-advisory"
                    }]
    }))
    .unwrap()
}

async fn spawn_pypi_mock() -> Url {
    let app = osv_router(
        |queries| {
            assert_eq!(queries[0].package.ecosystem.as_deref(), Some("PyPI"));
            assert_eq!(queries[0].package.name.as_deref(), Some("requests-test"));
            serde_json::from_value(serde_json::json!({
                "results": [{"vulns": [{"id": "PYSEC-2026-1"}]}]
            }))
            .unwrap()
        },
        [pypi_doc()],
    );
    spawn(app).await
}

#[tokio::test]
async fn short_pypi_batch_response_fails_the_audit() {
    let app = Router::new().route(
        "/v1/querybatch",
        post(|| async { Json(serde_json::json!({"results": []})) }),
    );
    let osv_client = test_osv_client(spawn(app).await);
    let packages = vec![AuditPackage {
        name: pypi_name("requests"),
        version: Some("2.32.0".to_string()),
        ecosystem: PackageEcosystem::Pypi,
        environments: vec!["default".to_string()],
    }];

    let err = super::audit(
        &test_client(spawn_mock().await),
        &osv_client,
        packages,
        &[],
        ThreatSources::default(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, AuditError::UnexpectedResponse { .. }),
        "unexpected error: {err}"
    );
}

#[test]
fn package_rules_match_pypi_names_under_pep_503() {
    let packages = [AuditPackage {
        name: pypi_name("requests_test"),
        version: Some("2.32.0".to_string()),
        ecosystem: PackageEcosystem::Pypi,
        environments: vec!["default".to_string()],
    }];
    let rules = [IgnoreRule::new(
        None,
        IgnoreScope {
            package: Some("Requests.Test".to_string()),
            ..IgnoreScope::default()
        },
        IgnoreMode::Permanent,
    )
    .unwrap()];
    let planned: Vec<_> = packages.iter().filter_map(plan_query).collect();
    let results = [serde_json::from_value(serde_json::json!({
        "vulns": [{"id": "PYSEC-2026-1"}]
    }))
    .unwrap()];
    let documents = HashMap::from([((Database::Pypi, "PYSEC-2026-1".to_string()), pypi_doc())]);
    let report = assemble_report(&planned, &results, &documents, &rules).unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.ignored.len(), 1);
    assert_eq!(report.ignored[0].ignore_mode, Some(IgnoreMode::Permanent));
}

#[tokio::test]
async fn pypi_package_is_audited_with_osv() {
    let conda_base = spawn_mock().await;
    let osv_client = test_osv_client(spawn_pypi_mock().await);
    let packages = vec![AuditPackage {
        name: pypi_name("requests_test"),
        version: Some("2.32.0".to_string()),
        ecosystem: PackageEcosystem::Pypi,
        environments: vec!["default".to_string()],
    }];
    let report = super::audit(
        &test_client(conda_base),
        &osv_client,
        packages,
        &[],
        ThreatSources::default(),
    )
    .await
    .unwrap();

    assert_eq!(report.summary.audited, 1);
    assert_eq!(report.matches.len(), 1);
    assert!(report.not_audited.is_empty());
    let vuln_match = &report.matches[0];
    assert_eq!(vuln_match.package.name.as_str(), "requests-test");
    assert_eq!(vuln_match.package.ecosystem, PackageEcosystem::Pypi);
    assert_eq!(vuln_match.id, "PYSEC-2026-1");
    assert_eq!(
        vuln_match.summary.as_deref(),
        Some("Python package vulnerability")
    );
    assert_eq!(vuln_match.aliases, ["CVE-2026-9999"]);
    assert_eq!(vuln_match.fixed_versions, ["2.32.1"]);
    // PyPI advisories are scored from their OSV CVSS vectors like any other.
    assert_eq!(vuln_match.severity, SeverityRating::Medium);
    let cvss_base_score = vuln_match.cvss_base_score.unwrap();
    assert!(
        (cvss_base_score - 5.3).abs() < 1e-9,
        "cvss base score: {cvss_base_score}"
    );
    assert_eq!(
        vuln_match.url.as_deref(),
        Some("https://example.com/pypi-advisory")
    );
}

/// Like conda-forge `pnpm`, which is npm's pnpm: matched through the npm
/// purl, so the npm fixes are other-ecosystem fixes.
fn vuln_doc_purl_matched() -> serde_json::Value {
    serde_json::json!({
        "id": "CVE-2026-50014",
        "summary": "Fixed on npm",
        "affected": [
            {
                "package": {"ecosystem": "npm", "name": "openssl", "purl": "pkg:npm/openssl"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "10.34.0"}]}]
            },
            {
                "package": {"ecosystem": "conda-forge", "name": "openssl"},
                "versions": ["3.1.0"],
                "database_specific": {"basilisk": {
                    "applicability": "evaluated",
                    "comparator": "semver",
                    "matcher_version": "v8",
                    "matched_identity": {"kind": "purl", "value": "pkg:npm/openssl"}
                }}
            }
        ]
    })
}

/// Like conda-forge `libpq` and CVE-2022-25852: a CPE match without
/// version bounds, while the same-name npm entry is another package.
fn vuln_doc_cpe_unbounded() -> serde_json::Value {
    serde_json::json!({
        "id": "CVE-2022-25852",
        "summary": "About the npm bindings",
        "affected": [
            {
                "package": {"ecosystem": "npm", "name": "openssl", "purl": "pkg:npm/openssl"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "1.8.10"}]}]
            },
            {
                "package": {"ecosystem": "conda-forge", "name": "openssl"},
                "versions": ["3.1.0"],
                "database_specific": {"basilisk": {
                    "applicability": "version_unbounded",
                    "comparator": "conda",
                    "matcher_version": "v8",
                    "matched_identity": {"kind": "cpe", "value": "cpe:2.3:a:openssl:openssl:*:*:*:*:*:*:*:*"}
                }}
            }
        ]
    })
}

#[tokio::test]
async fn purl_matched_match_keeps_other_ecosystem_fixes_and_reports_its_confidence() {
    let base = spawn_mock_serving(vuln_doc_purl_matched()).await;
    let report = audit(&test_client(base), packages(), &[]).await.unwrap();

    assert_eq!(report.matches.len(), 1);
    let vuln_match = &report.matches[0];
    assert!(vuln_match.fixed_versions.is_empty());
    assert_eq!(
        vuln_match.other_ecosystem_fixes,
        vec![OtherEcosystemFix {
            version: "10.34.0".to_string(),
            ecosystem: "npm".to_string(),
        }]
    );
    assert!(vuln_match.upstream_releases.is_empty());
    assert_eq!(vuln_match.match_confidence, MatchConfidence::Evaluated);
    assert_eq!(
        vuln_match.matched_identity,
        Some(MatchedIdentity {
            kind: MatchedIdentityKind::Purl,
            value: "pkg:npm/openssl".to_string(),
        })
    );
    let json = serde_json::to_value(vuln_match).unwrap();
    assert_eq!(
        json["matched_identity"],
        serde_json::json!({"kind": "purl", "value": "pkg:npm/openssl"})
    );
}

#[tokio::test]
async fn cpe_matched_match_has_no_other_ecosystem_fixes() {
    let base = spawn_mock_serving(vuln_doc_cpe_unbounded()).await;
    let report = audit(&test_client(base), packages(), &[]).await.unwrap();

    assert_eq!(report.matches.len(), 1);
    let vuln_match = &report.matches[0];
    assert!(vuln_match.fixed_versions.is_empty());
    assert!(
        vuln_match.other_ecosystem_fixes.is_empty(),
        "npm openssl is not conda-forge openssl: {:?}",
        vuln_match.other_ecosystem_fixes
    );
    assert!(vuln_match.upstream_releases.is_empty());
    assert_eq!(
        vuln_match.match_confidence,
        MatchConfidence::VersionUnbounded
    );
    assert_eq!(
        vuln_match.matched_identity.as_ref().map(|i| i.kind),
        Some(MatchedIdentityKind::Cpe)
    );
    let json = serde_json::to_value(vuln_match).unwrap();
    assert_eq!(json["match_confidence"], "version-unbounded");
    assert_eq!(json["matched_identity"]["kind"], "cpe");

    // With no fix known anywhere, until-fixed ignore applies.
    let base = spawn_mock_serving(vuln_doc_cpe_unbounded()).await;
    let rules = [IgnoreRule::from_id(
        "CVE-2022-25852".to_string(),
        IgnoreMode::UntilFixed,
    )];
    let report = audit(&test_client(base), packages(), ignore_until_fixed(&rules))
        .await
        .unwrap();
    assert!(report.matches.is_empty());
    assert_eq!(report.ignored.len(), 1);
}

#[tokio::test]
async fn match_without_match_metadata_are_unspecified() {
    // A plain OSV document from basilisk, and a PyPI vulnerability match from OSV.dev.
    let base = spawn_mock().await;
    let report = audit(&test_client(base), packages(), &[]).await.unwrap();
    let vuln_match = &report.matches[0];
    assert_eq!(vuln_match.match_confidence, MatchConfidence::Unspecified);
    assert_eq!(vuln_match.matched_identity, None);
    let json = serde_json::to_value(vuln_match).unwrap();
    assert_eq!(json["match_confidence"], "unspecified");
    assert_eq!(json["matched_identity"], serde_json::Value::Null);

    let osv_client = test_osv_client(spawn_pypi_mock().await);
    let packages = vec![AuditPackage {
        name: pypi_name("requests_test"),
        version: Some("2.32.0".to_string()),
        ecosystem: PackageEcosystem::Pypi,
        environments: vec!["default".to_string()],
    }];
    let report = super::audit(
        &test_client(spawn_mock().await),
        &osv_client,
        packages,
        &[],
        ThreatSources::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        report.matches[0].match_confidence,
        MatchConfidence::Unspecified
    );
    assert_eq!(report.matches[0].matched_identity, None);
}

/// EPSS and KEV feeds where CVE-2026-1234 (BSLK-1's alias) is scored and
/// KEV-listed and nothing else is. The KEV catalog is served at `kev.json`.
async fn spawn_threat_mock() -> Url {
    let app = epss_router(vec![serde_json::json!({
        "cve": "CVE-2026-1234",
        "epss": "0.500000000",
        "percentile": "0.900000000",
        "date": "2026-09-01"
    })])
    .merge(kev_router(serde_json::json!({
        "dateReleased": "2026-09-01T19:22:46.2162Z",
        "vulnerabilities": [{
            "cveID": "CVE-2026-1234",
            "vendorProject": "OpenSSL",
            "product": "OpenSSL",
            "vulnerabilityName": "OpenSSL buffer overflow",
            "dateAdded": "2026-08-01",
            "requiredAction": "Apply updates per vendor instructions.",
            "dueDate": "2026-08-22",
            "knownRansomwareCampaignUse": "Unknown",
            "notes": ""
        }]
    })));
    spawn(app).await
}

fn threat_clients(base: &Url) -> (EpssClient, KevClient) {
    let http = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
    (
        EpssClient::new(http.clone(), base.clone()),
        KevClient::new(http, base.join("kev.json").unwrap()),
    )
}

#[tokio::test]
async fn threat_data_is_attached_to_match_and_ranks_them() {
    let base = spawn_mock().await;
    let (epss_client, kev_client) = threat_clients(&spawn_threat_mock().await);
    let threat = ThreatSources {
        epss: Some(&epss_client),
        kev: Some(&kev_client),
    };
    let osv_client = clean_osv_client().await;
    let report = super::audit(&test_client(base), &osv_client, packages(), &[], threat)
        .await
        .unwrap();

    assert_eq!(
        report.threat_data,
        ThreatDataStatus {
            epss: ThreatSourceStatus::Loaded {
                date: Some("2026-09-01".to_string())
            },
            kev: ThreatSourceStatus::Loaded {
                date: Some("2026-09-01".to_string())
            },
        }
    );
    let vuln_match = &report.matches[0];
    assert_eq!(vuln_match.epss.len(), 1);
    assert_eq!(vuln_match.epss[0].cve, "CVE-2026-1234");
    assert_eq!(vuln_match.epss[0].epss, 0.5);
    assert_eq!(vuln_match.epss[0].percentile, 0.9);
    assert_eq!(vuln_match.kev.len(), 1);
    assert_eq!(vuln_match.kev[0].vendor_project, "OpenSSL");
    assert_eq!(
        vuln_match.kev[0].known_ransomware_campaign_use,
        RansomwareCampaignUse::Unknown
    );
    // Critical (9.0) averaged with CVSS 9.8 => 0.94; KEV-listed so threat
    // is 1.0 and the boost 1.05 => 98.7.
    assert_eq!(vuln_match.cvss_base_score, Some(9.8));
    assert!(
        (vuln_match.risk.value() - 98.7).abs() < 1e-9,
        "risk: {}",
        vuln_match.risk.value()
    );

    // Ignored vulnerability matches are enriched too.
    let rules = [IgnoreRule::from_id(
        "BSLK-1".to_string(),
        IgnoreMode::Permanent,
    )];
    let base = spawn_mock().await;
    let report = super::audit(&test_client(base), &osv_client, packages(), &rules, threat)
        .await
        .unwrap();
    assert_eq!(report.ignored[0].epss.len(), 1);
    assert_eq!(report.ignored[0].kev.len(), 1);
}

#[tokio::test]
async fn unreachable_threat_sources_degrade_to_a_status() {
    let base = spawn_mock().await;
    // A server without the feeds: every threat request is a 404.
    let (epss_client, kev_client) = threat_clients(&spawn_mock().await);
    let threat = ThreatSources {
        epss: Some(&epss_client),
        kev: Some(&kev_client),
    };
    let osv_client = clean_osv_client().await;
    let report = super::audit(&test_client(base), &osv_client, packages(), &[], threat)
        .await
        .unwrap();

    assert_eq!(report.matches.len(), 1);
    let vuln_match = &report.matches[0];
    assert!(vuln_match.epss.is_empty());
    assert!(vuln_match.kev.is_empty());
    assert_eq!(vuln_match.risk, RiskScore::NONE);
    for status in [&report.threat_data.epss, &report.threat_data.kev] {
        match status {
            ThreatSourceStatus::Failed { error } => {
                assert!(error.contains("404"), "error: {error}")
            }
            ThreatSourceStatus::Disabled | ThreatSourceStatus::Loaded { .. } => {
                panic!("expected a failed status, got {status:?}")
            }
        }
    }
}

#[tokio::test]
async fn match_are_ordered_by_risk_then_severity() {
    // Two advisories on openssl: BSLK-1 (critical, scored) and BSLK-NOFIX
    // (critical, no CVE score because its alias is unscored).
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
                            serde_json::json!({"vulns": [{"id": "BSLK-NOFIX"}, {"id": "BSLK-1"}]})
                        } else {
                            serde_json::json!({})
                        }
                    })
                    .collect();
                Json(serde_json::json!({"results": results}))
            }),
        )
        .route(
            "/v1/vulns/{id}",
            get(
                |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    if id == "BSLK-1" {
                        Json(vuln_doc())
                    } else {
                        Json(vuln_doc_nofix())
                    }
                },
            ),
        );
    let base = spawn(app).await;
    let (epss_client, kev_client) = threat_clients(&spawn_threat_mock().await);
    let threat = ThreatSources {
        epss: Some(&epss_client),
        kev: Some(&kev_client),
    };
    let osv_client = clean_osv_client().await;
    let mut report = super::audit(&test_client(base), &osv_client, packages(), &[], threat)
        .await
        .unwrap();

    let ids = |report: &AuditReport| -> Vec<String> {
        report
            .matches
            .iter()
            .map(|vuln_match| vuln_match.id.clone())
            .collect()
    };
    // Default: risk first, so the scored advisory leads.
    assert_eq!(ids(&report), ["BSLK-1", "BSLK-NOFIX"]);
    // Vulnerability id order is alphabetical.
    report.sort(SortKey::Vulnerability);
    assert_eq!(ids(&report), ["BSLK-1", "BSLK-NOFIX"]);
    // Same severity everywhere, so severity order falls through to risk.
    report.sort(SortKey::Severity);
    assert_eq!(ids(&report), ["BSLK-1", "BSLK-NOFIX"]);
    // KEV-listed first; the unlisted one follows regardless of risk.
    report.sort(SortKey::Kev);
    assert_eq!(ids(&report), ["BSLK-1", "BSLK-NOFIX"]);
}

#[test]
fn advisory_aliases_count_once_before_ignore() {
    let packages = packages();
    let planned: Vec<_> = packages.iter().filter_map(plan_query).collect();
    let ids = [
        "PYSEC-1",
        "CVE-1",
        "BSLK-OTHER",
        "GHSA-1",
        "BSLK-OTHER-ALIAS",
    ];
    let results = planned
        .iter()
        .map(|query| {
            if query.package.name.as_str() == "openssl" {
                serde_json::from_value(serde_json::json!({
                    "vulns": ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>()
                }))
                .unwrap()
            } else {
                BatchResult::default()
            }
        })
        .collect::<Vec<_>>();
    let documents = ids
        .into_iter()
        .map(|id| {
            let (aliases, fixes) = match id {
                "PYSEC-1" => (vec!["GHSA-1"], vec![]),
                "GHSA-1" => (vec!["CVE-1"], vec!["3.1.1"]),
                "CVE-1" => (vec![], vec!["3.1.2"]),
                "BSLK-OTHER-ALIAS" => (vec!["BSLK-OTHER"], vec![]),
                _ => (vec![], vec![]),
            };
            let doc = serde_json::from_value(serde_json::json!({
                "id": id,
                "aliases": aliases,
                "affected": [{
                    "package": {"ecosystem": "conda-forge", "name": "openssl"},
                    "ranges": fixes.iter().map(|fix| serde_json::json!({
                        "type": "ECOSYSTEM",
                        "events": [{"introduced": "0"}, {"fixed": fix}]
                    })).collect::<Vec<_>>()
                }]
            }))
            .unwrap();
            ((Database::Basilisk, id.to_string()), doc)
        })
        .collect();
    let rules = [IgnoreRule::from_id(
        "CVE-1".to_string(),
        IgnoreMode::Permanent,
    )];
    let spec = &rules;
    let report = assemble_report(&planned, &results, &documents, spec).unwrap();
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].id, "BSLK-OTHER");
    assert_eq!(report.matches[0].aliases, ["BSLK-OTHER-ALIAS"]);
    assert_eq!(report.summary.ignored, 1);
    assert!(report.unused_ignores.is_empty());
    let vuln_match = &report.ignored[0];
    assert_eq!(vuln_match.id, "PYSEC-1");
    assert_eq!(vuln_match.aliases, ["CVE-1", "GHSA-1"]);
    assert_eq!(vuln_match.fixed_versions, ["3.1.1", "3.1.2"]);
}
