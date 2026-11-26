use super::*;
use crate::test_support::{conda_name, pypi_name};

fn on_pypi(version: &str) -> OtherEcosystemFix {
    OtherEcosystemFix {
        version: version.to_string(),
        ecosystem: "PyPI".to_string(),
    }
}

fn project_release(version: &str, conda_forge_build: CondaForgeBuild) -> UpstreamRelease {
    UpstreamRelease {
        version: version.to_string(),
        conda_forge_build,
    }
}

/// Every fix version reported outside the queried ecosystem.
fn external_versions(fixes: &FixedVersions) -> Vec<&str> {
    fixes
        .other_ecosystem
        .iter()
        .map(|fix| fix.version.as_str())
        .chain(
            fixes
                .upstream_releases
                .iter()
                .map(|fix| fix.version.as_str()),
        )
        .collect()
}
/// A document shaped like basilisk's real output: the conda-forge entry
/// carries `versions` + `database_specific` (no ranges); the PyPI
/// entry carries the ranges with `fixed` events.
fn basilisk_shaped_doc() -> OsvVulnerability {
    serde_json::from_value(serde_json::json!({
            "id": "CVE-2026-9999",
            "affected": [
                {
                    "package": {"ecosystem": "PyPI", "name": "pillow", "purl": "pkg:pypi/pillow"},
                    "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "10.2.0"}]}]
                },
                {
                    "package": {"ecosystem": "PyPI", "name": "django"},
                    "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "5.0.2"}]}]
                },
                {
                    "package": {
                        "ecosystem": "conda-forge",
                        "name": "pillow",
                        "purl": "pkg:conda/conda-forge/pillow"
                    },
                    "versions": ["10.0.0", "10.1.0"],
                    "database_specific": {"basilisk": {"applicability": "evaluated"}}
                }
            ]
        }))
        .unwrap()
}

fn basilisk_fix_doc(conda_publication: &str) -> OsvVulnerability {
    serde_json::from_value(serde_json::json!({
        "id": "CVE-2026-50219",
        "affected": [{
            "package": {
                "ecosystem": "conda-forge",
                "name": "libexpat",
                "purl": "pkg:conda/conda-forge/libexpat"
            },
            "versions": ["2.8.0", "2.8.1"],
            "database_specific": {
                "basilisk": {
                    "applicability": "evaluated",
                    "comparator": "conda",
                    "matcher_version": "nvd_cpe@3",
                    "fix_version": "2.8.2",
                    "conda_publication": conda_publication,
                    "reported_fixes": [{
                        "source_kind": "nvd",
                        "source_advisory_id": "CVE-2026-50219",
                        "reported_fix": "9.9.9",
                        "conda_publication": "published"
                    }]
                }
            }
        }]
    }))
    .unwrap()
}

#[test]
fn basilisk_fix_is_classified_by_publication_state() {
    for (publication, expected_fixed, expected_releases) in [
        ("published", vec!["2.8.2"], vec![]),
        (
            "not_published",
            vec![],
            vec![project_release("2.8.2", CondaForgeBuild::Missing)],
        ),
        (
            "unknown",
            vec![],
            vec![project_release("2.8.2", CondaForgeBuild::Unknown)],
        ),
    ] {
        let fixes = fixed_versions(
            &basilisk_fix_doc(publication),
            &conda_name("libexpat"),
            "conda-forge",
        );
        assert!(!external_versions(&fixes).contains(&"9.9.9"));
        let FixedVersions {
            same_ecosystem: fixed,
            other_ecosystem,
            upstream_releases,
        } = fixes;
        assert_eq!(fixed, expected_fixed, "publication: {publication}");
        assert!(other_ecosystem.is_empty(), "publication: {publication}");
        assert_eq!(
            upstream_releases, expected_releases,
            "publication: {publication}"
        );
    }
}

#[test]
fn fixed_versions_only_come_from_the_queried_ecosystem() {
    let doc = basilisk_shaped_doc();
    // conda-forge entries carry no fixed events; the PyPI fix must not be
    // presented as a conda-forge fixed version.
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("pillow"), "conda-forge");
    assert!(fixed.is_empty());
    assert_eq!(other_ecosystem, vec![on_pypi("10.2.0")]);
    assert!(upstream_releases.is_empty());

    // Queried as PyPI, the same fix IS a same-ecosystem fixed version.
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &pypi_name("pillow"), "PyPI");
    assert_eq!(fixed, vec!["10.2.0"]);
    assert!(other_ecosystem.is_empty());
    assert!(upstream_releases.is_empty());
}

#[test]
fn missing_or_malformed_basilisk_metadata_falls_back_to_osv_ranges() {
    let metadata_values = [
        None,
        Some(serde_json::json!({"basilisk": {"fix_version": "2.8.2"}})),
        Some(serde_json::json!({
            "basilisk": {
                "fix_version": 282,
                "conda_publication": "published"
            }
        })),
        Some(serde_json::json!({
            "basilisk": {
                "fix_version": "2.8.2",
                "conda_publication": "sometimes"
            }
        })),
    ];

    for database_specific in metadata_values {
        let mut affected = serde_json::json!({
            "package": {"ecosystem": "conda-forge", "name": "libexpat"},
            "ranges": [{
                "type": "ECOSYSTEM",
                "events": [{"introduced": "0"}, {"fixed": "2.8.3"}]
            }]
        });
        if let Some(database_specific) = database_specific {
            affected["database_specific"] = database_specific;
        }
        let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
            "id": "CVE-2026-50219",
            "affected": [affected]
        }))
        .unwrap();

        let FixedVersions {
            same_ecosystem: fixed,
            other_ecosystem,
            upstream_releases,
        } = fixed_versions(&doc, &conda_name("libexpat"), "conda-forge");
        assert_eq!(fixed, vec!["2.8.3"]);
        assert!(other_ecosystem.is_empty());
        assert!(upstream_releases.is_empty());
    }
}

#[test]
fn basilisk_metadata_requires_matching_package_and_ecosystem() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "CVE-2026-50219",
        "affected": [
            {
                "package": {"ecosystem": "conda-forge", "name": "other"},
                "database_specific": {"basilisk": {
                    "fix_version": "1.0.0",
                    "conda_publication": "published"
                }}
            },
            {
                "package": {"ecosystem": "PyPI", "name": "libexpat"},
                "database_specific": {"basilisk": {
                    "fix_version": "2.0.0",
                    "conda_publication": "not_published"
                }}
            },
            {
                "database_specific": {"basilisk": {
                    "fix_version": "3.0.0",
                    "conda_publication": "published"
                }}
            }
        ]
    }))
    .unwrap();

    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("libexpat"), "conda-forge");
    assert!(fixed.is_empty());
    assert!(other_ecosystem.is_empty());
    assert!(upstream_releases.is_empty());
}

#[test]
fn non_conda_queries_ignore_basilisk_metadata_but_keep_osv_ranges() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "PYSEC-2026-1",
        "affected": [{
            "package": {"ecosystem": "PyPI", "name": "demo"},
            "ranges": [{"type": "ECOSYSTEM", "events": [{"fixed": "4.0.0"}]}],
            "database_specific": {"basilisk": {
                "fix_version": "3.0.0",
                "conda_publication": "published"
            }}
        }]
    }))
    .unwrap();

    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &pypi_name("demo"), "PyPI");
    assert_eq!(fixed, vec!["4.0.0"]);
    assert!(other_ecosystem.is_empty());
    assert!(upstream_releases.is_empty());
}

#[test]
fn standard_range_fixes_and_basilisk_metadata_are_deduplicated() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "CVE-2026-50219",
        "affected": [
            {
                "package": {"ecosystem": "conda-forge", "name": "libexpat"},
                "ranges": [{"type": "ECOSYSTEM", "events": [
                    {"fixed": "2.8.2"},
                    {"fixed": "2.8.2"},
                    {"fixed": "2.8.3"}
                ]}],
                "database_specific": {"basilisk": {
                    "applicability": "evaluated",
                    "fix_version": "2.8.2",
                    "conda_publication": "published"
                }}
            },
            {
                "package": {"ecosystem": "PyPI", "name": "libexpat"},
                "ranges": [{"type": "ECOSYSTEM", "events": [
                    {"fixed": "2.8.2"},
                    {"fixed": "2.8.4"},
                    {"fixed": "2.8.4"}
                ]}]
            }
        ]
    }))
    .unwrap();

    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("libexpat"), "conda-forge");
    assert_eq!(fixed, vec!["2.8.2", "2.8.3"]);
    assert_eq!(other_ecosystem, vec![on_pypi("2.8.4")]);
    assert!(upstream_releases.is_empty());
}

#[test]
fn only_release_ranges_contribute_fixed_versions() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "GHSA-1",
        "affected": [{
            "package": {"ecosystem": "PyPI", "name": "demo"},
            "ranges": [
                {"type": "GIT", "events": [{"fixed": "deadbeef"}]},
                {"type": "SEMVER", "events": [{"fixed": "1.2.3"}]}
            ]
        }]
    }))
    .unwrap();

    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &pypi_name("demo"), "PyPI");
    assert_eq!(fixed, vec!["1.2.3"]);
    assert!(other_ecosystem.is_empty());
    assert!(upstream_releases.is_empty());
}

#[test]
fn other_ecosystem_fixed_versions_require_a_name_match() {
    // The django entry in the same advisory must not leak its fixed
    // version into pillow's report.
    let doc = basilisk_shaped_doc();
    let fixes = fixed_versions(&doc, &conda_name("pillow"), "conda-forge");
    assert!(!external_versions(&fixes).contains(&"5.0.2"));
    let fixed = fixes.same_ecosystem;
    assert!(!fixed.contains(&"5.0.2".to_string()));
}

#[test]
fn fix_present_in_own_ecosystem_is_not_repeated_elsewhere() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
            "id": "X",
            "affected": [
                {
                    "package": {"ecosystem": "conda-forge", "name": "pillow"},
                    "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "10.2.0"}]}]
                },
                {
                    "package": {"ecosystem": "PyPI", "name": "pillow"},
                    "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "10.2.0"}]}]
                }
            ]
        }))
        .unwrap();
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("pillow"), "conda-forge");
    assert_eq!(fixed, vec!["10.2.0"]);
    assert!(
        other_ecosystem.is_empty() && upstream_releases.is_empty(),
        "version already reported as fixed must not repeat elsewhere: {other_ecosystem:?} {upstream_releases:?}"
    );
}

#[test]
fn basilisk_advisory_url_uses_the_explorer_instead_of_source_references() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "CVE-2026-50219",
        "references": [
            {"type": "FIX", "url": "https://example.com/patch"},
            {"type": "REPORT", "url": "https://example.com/issue"},
            {"type": "ADVISORY", "url": "https://example.com/advisory"}
        ]
    }))
    .unwrap();

    assert_eq!(
        basilisk_advisory_url(&doc),
        "https://basilisk.prefix.dev/advisories/CVE-2026-50219"
    );
}

#[test]
fn pypi_advisory_url_prefers_web_then_falls_back_to_osv() {
    let with_web: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "PYSEC-1",
        "references": [
            {"type": "FIX", "url": "https://example.com/fix"},
            {"type": "WEB", "url": "https://example.com/details"}
        ]
    }))
    .unwrap();
    assert_eq!(pypi_advisory_url(&with_web), "https://example.com/details");

    let without_references = OsvVulnerability {
        id: "PYSEC-2".to_string(),
        ..Default::default()
    };
    assert_eq!(
        pypi_advisory_url(&without_references),
        "https://osv.dev/vulnerability/PYSEC-2"
    );
}

#[test]
fn affected_entry_without_package_counts_as_same_ecosystem() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "X",
        "affected": [{
            "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "1.2.3"}]}]
        }]
    }))
    .unwrap();
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("anything"), "conda-forge");
    assert_eq!(fixed, vec!["1.2.3"]);
    assert!(other_ecosystem.is_empty());
    assert!(upstream_releases.is_empty());
}

#[test]
fn pypi_names_match_under_pep_503_normalization() {
    // OSV.dev keeps the advisory's spelling (`ruamel.yaml`); the lock file
    // and the query use the normalized `ruamel-yaml`. They are one package.
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "PYSEC-2026-2",
        "affected": [{
            "package": {"ecosystem": "PyPI", "name": "Ruamel.YAML"},
            "severity": [{
                "type": "CVSS_V3",
                "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"
            }],
            "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "0.18.7"}]}]
        }]
    }))
    .unwrap();
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &pypi_name("ruamel_yaml"), "PyPI");
    assert_eq!(fixed, vec!["0.18.7"]);
    assert!(other_ecosystem.is_empty());
    assert!(upstream_releases.is_empty());
    assert_eq!(
        severities(&doc, &pypi_name("ruamel_yaml"), "PyPI").count(),
        1
    );

    // Conda names only fold case; `ruamel.yaml` and `ruamel-yaml` differ.
    let FixedVersions {
        same_ecosystem: fixed,
        ..
    } = fixed_versions(&doc, &conda_name("ruamel-yaml"), "PyPI");
    assert!(fixed.is_empty());
    assert_eq!(
        severities(&doc, &conda_name("ruamel-yaml"), "PyPI").count(),
        0
    );
}

fn on_npm(version: &str) -> OtherEcosystemFix {
    OtherEcosystemFix {
        version: version.to_string(),
        ecosystem: "npm".to_string(),
    }
}

fn purl_identity(value: &str) -> MatchedIdentity {
    MatchedIdentity {
        kind: MatchedIdentityKind::Purl,
        value: value.to_string(),
    }
}

fn cpe_identity(value: &str) -> MatchedIdentity {
    MatchedIdentity {
        kind: MatchedIdentityKind::Cpe,
        value: value.to_string(),
    }
}

/// Shaped like CVE-2026-50014 for conda-forge `pnpm`, which really is
/// npm's pnpm: basilisk matched it through the npm purl and evaluated the
/// versions; the npm entries carry the fixes.
fn pnpm_shaped_doc() -> OsvVulnerability {
    serde_json::from_value(serde_json::json!({
        "id": "CVE-2026-50014",
        "affected": [
            {
                "package": {"ecosystem": "npm", "name": "pnpm", "purl": "pkg:npm/pnpm"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "10.34.0"}]}]
            },
            {
                "package": {"ecosystem": "npm", "name": "pnpm", "purl": "pkg:npm/pnpm@11"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "11.0.0"}, {"fixed": "11.4.0"}]}]
            },
            {
                "package": {"ecosystem": "conda-forge", "name": "pnpm", "purl": "pkg:conda/conda-forge/pnpm"},
                "versions": ["10.33.0"],
                "database_specific": {"basilisk": {
                    "applicability": "evaluated",
                    "comparator": "semver",
                    "conda_publication": "not_published",
                    "fix_version": "11.4.0",
                    "matcher_version": "v8",
                    "matched_identity": {"kind": "purl", "value": "pkg:npm/pnpm"},
                    "reported_fixes": []
                }}
            }
        ]
    }))
    .unwrap()
}

/// Shaped like CVE-2022-25852 for conda-forge `libpq` (PostgreSQL's C
/// library): a CPE match with no version bounds, while the advisory is
/// about the npm bindings of the same name.
fn libpq_shaped_doc(basilisk: serde_json::Value) -> OsvVulnerability {
    serde_json::from_value(serde_json::json!({
        "id": "CVE-2022-25852",
        "affected": [
            {
                "package": {"ecosystem": "npm", "name": "pg-native", "purl": "pkg:npm/pg-native"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "3.0.1"}]}]
            },
            {
                "package": {"ecosystem": "npm", "name": "libpq", "purl": "pkg:npm/libpq"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "1.8.10"}]}]
            },
            {
                "package": {"ecosystem": "conda-forge", "name": "libpq", "purl": "pkg:conda/conda-forge/libpq"},
                "versions": ["16.0"],
                "database_specific": {"basilisk": basilisk}
            }
        ]
    }))
    .unwrap()
}

#[test]
fn affected_severity_respects_basilisk_package_identity() {
    let critical = "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H";
    let medium = "CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:U/C:H/I:N/A:N";
    for (metadata, expected) in [
        (
            serde_json::json!({
                "applicability": "evaluated",
                "matched_identity": {
                    "kind": "purl",
                    "value": "pkg:npm/pg-native@3.0.0?arch=x86"
                }
            }),
            vec![critical],
        ),
        (
            serde_json::json!({
                "applicability": "evaluated",
                "matched_identity": {
                    "kind": "cpe",
                    "value": "cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*"
                }
            }),
            vec![],
        ),
        (
            serde_json::json!({"applicability": "evaluated"}),
            vec![medium],
        ),
        (serde_json::json!({"applicability": "unevaluated"}), vec![]),
    ] {
        let mut doc = libpq_shaped_doc(metadata.clone());
        for (affected, score) in doc.affected.iter_mut().zip([critical, medium]) {
            affected.severity.push(OsvSeverity {
                kind: "CVSS_V3".to_string(),
                score: score.to_string(),
                source: None,
            });
        }
        let name = conda_name("libpq");
        let scores: Vec<_> = severities(&doc, &name, "conda-forge")
            .map(|severity| severity.score.as_str())
            .collect();
        assert_eq!(scores, expected, "metadata: {metadata}");
    }
}

#[test]
fn purl_identity_admits_fixes_from_that_purl_only() {
    let doc = pnpm_shaped_doc();
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("pnpm"), "conda-forge");
    assert!(fixed.is_empty());
    // The version suffix on the second entry's purl does not matter, and
    // the npm origin of 11.4.0 wins over basilisk's project release.
    assert_eq!(other_ecosystem, vec![on_npm("10.34.0"), on_npm("11.4.0")]);
    assert!(upstream_releases.is_empty());

    assert_eq!(
        basilisk_match(&doc, &conda_name("pnpm"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::Evaluated,
            matched_identity: Some(purl_identity("pkg:npm/pnpm")),
        })
    );
}

#[test]
fn purl_identity_matches_by_purl_not_by_name() {
    // Basilisk matched libpq through the pg-native purl: only that entry's
    // fix counts, even though its name differs; the same-name npm entry's
    // fix does not.
    let doc = libpq_shaped_doc(serde_json::json!({
        "applicability": "evaluated",
        "matched_identity": {"kind": "purl", "value": "pkg:npm/pg-native@3.0.0?arch=x86"}
    }));
    let FixedVersions {
        other_ecosystem, ..
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert_eq!(other_ecosystem, vec![on_npm("3.0.1")]);
}

#[test]
fn cpe_identity_admits_no_cross_ecosystem_fixes() {
    let doc = libpq_shaped_doc(serde_json::json!({
        "applicability": "version_unbounded",
        "comparator": "conda",
        "matcher_version": "v8",
        "matched_identity": {"kind": "cpe", "value": "cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*"}
    }));
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert!(fixed.is_empty());
    assert!(upstream_releases.is_empty());
    assert!(
        other_ecosystem.is_empty(),
        "npm libpq is not conda-forge libpq: {other_ecosystem:?}"
    );
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::VersionUnbounded,
            matched_identity: Some(cpe_identity("cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*")),
        })
    );
}

#[test]
fn cpe_identity_keeps_basilisk_project_fix() {
    let doc = libpq_shaped_doc(serde_json::json!({
        "applicability": "evaluated",
        "fix_version": "16.1",
        "conda_publication": "not_published",
        "matched_identity": {"kind": "cpe", "value": "cpe:2.3:a:postgresql:postgresql:*:*:*:*:*:*:*:*"}
    }));
    let FixedVersions {
        upstream_releases, ..
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert_eq!(
        upstream_releases,
        vec![project_release("16.1", CondaForgeBuild::Missing)]
    );
}

#[test]
fn without_identity_the_name_match_needs_an_evaluated_applicability() {
    // Live documents predate `matched_identity`. The same-name npm fix is
    // trusted only when basilisk evaluated the conda-forge match.
    for (applicability, expected) in [
        ("evaluated", vec![on_npm("1.8.10")]),
        ("unevaluated", vec![]),
        ("version_unbounded", vec![]),
        ("something_new", vec![]),
    ] {
        let doc = libpq_shaped_doc(serde_json::json!({
            "applicability": applicability,
            "comparator": "conda",
            "matcher_version": "v8"
        }));
        let FixedVersions {
            other_ecosystem, ..
        } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
        assert_eq!(other_ecosystem, expected, "applicability: {applicability}");
        assert_eq!(
            basilisk_match(&doc, &conda_name("libpq"), "conda-forge").map(|m| m.matched_identity),
            Some(None),
            "applicability: {applicability}"
        );
    }
}

#[test]
fn unknown_applicability_is_unspecified_and_trusts_nothing() {
    let doc = libpq_shaped_doc(serde_json::json!({"comparator": "conda"}));
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::Unspecified,
            matched_identity: None,
        })
    );
    let FixedVersions {
        other_ecosystem, ..
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert!(other_ecosystem.is_empty());

    let doc = libpq_shaped_doc(serde_json::json!({
        "applicability": "evaluated",
        "matched_identity": {"kind": "isbn", "value": "x"}
    }));
    let FixedVersions {
        other_ecosystem, ..
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    // An identity kind this version does not know is no identity at all.
    assert_eq!(other_ecosystem, vec![on_npm("1.8.10")]);
}

#[test]
fn plain_osv_documents_keep_the_name_match() {
    let doc: OsvVulnerability = serde_json::from_value(serde_json::json!({
        "id": "CVE-2022-25852",
        "affected": [
            {
                "package": {"ecosystem": "npm", "name": "libpq"},
                "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "1.8.10"}]}]
            },
            {
                "package": {"ecosystem": "conda-forge", "name": "libpq"},
                "versions": ["16.0"]
            }
        ]
    }))
    .unwrap();
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        None
    );
    let FixedVersions {
        other_ecosystem, ..
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert_eq!(other_ecosystem, vec![on_npm("1.8.10")]);
}

#[test]
fn match_metadata_is_only_read_for_conda_forge_queries() {
    let doc = pnpm_shaped_doc();
    assert_eq!(basilisk_match(&doc, &pypi_name("pnpm"), "PyPI"), None);
    assert_eq!(
        basilisk_match(&doc, &conda_name("other"), "conda-forge"),
        None
    );
}

#[test]
fn combined_matches_keep_identity_and_applicability_together() {
    let doc = libpq_shaped_doc(serde_json::json!({
        "matches": [
            {
                "applicability": "version_unbounded",
                "comparator": "conda",
                "matcher_version": "v8",
                "matched_identity": {"kind": "cpe", "value": "cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*"}
            },
            {
                "applicability": "unevaluated",
                "comparator": "semver",
                "matcher_version": "v8",
                "matched_identity": {"kind": "purl", "value": "pkg:npm/libpq"}
            }
        ]
    }));
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::Unevaluated,
            matched_identity: Some(purl_identity("pkg:npm/libpq")),
        })
    );
    let FixedVersions {
        other_ecosystem, ..
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert_eq!(other_ecosystem, vec![on_npm("1.8.10")]);

    // CPE-only matches stay CPE, and an empty array reads like no array.
    let doc = libpq_shaped_doc(serde_json::json!({
        "matches": [
            {"applicability": "version_unbounded", "matched_identity": {"kind": "cpe", "value": "cpe:a"}},
            {"applicability": "evaluated", "matched_identity": {"kind": "cpe", "value": "cpe:b"}}
        ]
    }));
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::Evaluated,
            matched_identity: Some(cpe_identity("cpe:b")),
        })
    );
    let doc = libpq_shaped_doc(serde_json::json!({
        "matches": [
            {"applicability": "evaluated", "matched_identity": {"kind": "cpe", "value": "cpe:b"}},
            {"applicability": "version_unbounded", "matched_identity": {"kind": "purl", "value": "pkg:npm/libpq"}}
        ]
    }));
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::Evaluated,
            matched_identity: Some(cpe_identity("cpe:b")),
        })
    );
    assert!(
        fixed_versions(&doc, &conda_name("libpq"), "conda-forge")
            .other_ecosystem
            .is_empty()
    );
    let doc = libpq_shaped_doc(serde_json::json!({"applicability": "evaluated", "matches": []}));
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::Evaluated,
            matched_identity: None,
        })
    );
}

#[test]
fn equally_applicable_matches_preserve_cpe_identity_in_both_orders() {
    let cpe = serde_json::json!({
        "applicability": "evaluated",
        "matched_identity": {"kind": "cpe", "value": "cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*"}
    });
    let without_identity = serde_json::json!({"applicability": "evaluated"});
    for matches in [[&cpe, &without_identity], [&without_identity, &cpe]] {
        let doc = libpq_shaped_doc(serde_json::json!({"matches": matches}));
        assert_eq!(
            basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
            Some(BasiliskMatch {
                match_confidence: MatchConfidence::Evaluated,
                matched_identity: Some(cpe_identity("cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*")),
            })
        );
        assert!(
            fixed_versions(&doc, &conda_name("libpq"), "conda-forge")
                .other_ecosystem
                .is_empty(),
            "a CPE match must not inherit the same-name npm package's fix"
        );
    }
}

#[test]
fn purl_keys_ignore_version_qualifiers_subpath_and_case() {
    for (purl, expected) in [
        ("pkg:npm/pnpm", "pkg:npm/pnpm"),
        ("pkg:npm/pnpm@11.4.0", "pkg:npm/pnpm"),
        ("pkg:npm/pnpm?arch=x86#src", "pkg:npm/pnpm"),
        ("pkg:npm/%40angular/core@1.0", "pkg:npm/@angular/core"),
        ("pkg:npm/@angular/core", "pkg:npm/@angular/core"),
        ("pkg:PyPI/Pillow/", "pkg:pypi/pillow"),
        (
            "pkg:conda/conda-forge/libpq@16.0?build=h1",
            "pkg:conda/conda-forge/libpq",
        ),
    ] {
        assert_eq!(purl_key(purl), expected, "purl: {purl}");
    }
}

#[test]
fn applicability_serializes_kebab_case_and_orders_by_confidence() {
    assert_eq!(
        serde_json::to_value([
            MatchConfidence::Unspecified,
            MatchConfidence::VersionUnbounded,
            MatchConfidence::Unevaluated,
            MatchConfidence::Evaluated,
        ])
        .unwrap(),
        serde_json::json!([
            "unspecified",
            "version-unbounded",
            "unevaluated",
            "evaluated"
        ])
    );
    assert!(MatchConfidence::Evaluated > MatchConfidence::Unevaluated);
    assert!(MatchConfidence::Unevaluated > MatchConfidence::VersionUnbounded);
    assert!(MatchConfidence::VersionUnbounded > MatchConfidence::Unspecified);
    // `unspecified` is never something basilisk says.
    assert!(serde_json::from_value::<MatchConfidence>(serde_json::json!("unspecified")).is_err());
}

#[test]
fn merged_form_keeps_fix_metadata_top_level_and_tolerates_null_comparator() {
    // Basilisk's merged form: per-match fields inside `matches`, the fix
    // metadata top-level, and `comparator` nullable.
    let doc = libpq_shaped_doc(serde_json::json!({
        "matches": [
            {
                "applicability": "version_unbounded",
                "comparator": null,
                "matcher_version": "v8",
                "matched_identity": {"kind": "cpe", "value": "cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*"}
            }
        ],
        "fix_version": "16.1",
        "conda_publication": "not_published",
        "reported_fixes": []
    }));
    assert_eq!(
        basilisk_match(&doc, &conda_name("libpq"), "conda-forge"),
        Some(BasiliskMatch {
            match_confidence: MatchConfidence::VersionUnbounded,
            matched_identity: Some(cpe_identity("cpe:2.3:a:postgresql:libpq:*:*:*:*:*:*:*:*")),
        })
    );
    let FixedVersions {
        same_ecosystem: fixed,
        other_ecosystem,
        upstream_releases,
    } = fixed_versions(&doc, &conda_name("libpq"), "conda-forge");
    assert!(fixed.is_empty());
    assert!(other_ecosystem.is_empty());
    assert_eq!(
        upstream_releases,
        vec![project_release("16.1", CondaForgeBuild::Missing)]
    );
}
