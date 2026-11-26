use super::*;
use crate::{
    AffectedRange, PackageEcosystem, PackageId, RangeSource, RiskScore, SeverityRating,
    report::IgnoreMode,
};

fn events(events: &[(&str, &str)]) -> AffectedRange {
    AffectedRange {
        source: RangeSource::Ecosystem,
        events: events
            .iter()
            .map(|(kind, version)| match *kind {
                "introduced" => RangeEvent::Introduced(version.to_string()),
                "fixed" => RangeEvent::Fixed(version.to_string()),
                "last_affected" => RangeEvent::LastAffected(version.to_string()),
                other => panic!("unknown event {other}"),
            })
            .collect(),
    }
}

fn record(id: &str, ranges: Vec<AffectedRange>, versions: &[&str]) -> AdvisoryEvidence {
    AdvisoryEvidence {
        id: id.to_string(),
        ranges,
        versions: versions.iter().map(|v| v.to_string()).collect(),
    }
}

fn finding(name: PackageName, version: &str, advisories: Vec<AdvisoryEvidence>) -> Match {
    let ecosystem = match name {
        PackageName::Conda(_) => PackageEcosystem::CondaForge,
        PackageName::Pypi(_) => PackageEcosystem::Pypi,
    };
    let fixed_versions = advisories
        .iter()
        .flat_map(|advisory| &advisory.ranges)
        .flat_map(|range| &range.events)
        .filter_map(|event| match event {
            RangeEvent::Fixed(version) => Some(version.clone()),
            _ => None,
        })
        .collect();
    Match {
        package: PackageId {
            name,
            version: version.to_string(),
            ecosystem,
        },
        environments: vec!["default".to_string()],
        id: advisories
            .first()
            .map_or_else(|| "CVE-0".to_string(), |a| a.id.clone()),
        aliases: vec![],
        severity: SeverityRating::High,
        cvss_base_score: None,
        fixed_versions,
        advisories,
        other_ecosystem_fixes: vec![],
        upstream_releases: vec![],
        match_confidence: MatchConfidence::Unspecified,
        matched_identity: None,
        summary: None,
        url: None,
        epss: vec![],
        kev: vec![],
        risk: RiskScore::NONE,
        ignore_mode: None,
    }
}

fn conda(name: &str) -> PackageName {
    PackageName::conda(name).unwrap()
}

fn pypi(name: &str) -> PackageName {
    PackageName::pypi(name).unwrap()
}

fn report(matches: Vec<Match>) -> AuditReport {
    AuditReport {
        matches,
        ..AuditReport::default()
    }
}

fn requirement(package: PackageName, spec: &str) -> ManifestRequirement {
    ManifestRequirement {
        package,
        spec: spec.to_string(),
    }
}

/// The plan's constraints, one per line.
fn planned(matches: Vec<Match>, requirements: &[ManifestRequirement]) -> String {
    match plan(&report(matches), &Policy::default(), requirements) {
        Ok(plan) => plan
            .constraints
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        Err(blockers) => blockers
            .iter()
            .map(|blocker| format!("blocked: {blocker}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn two_branches(id: &str) -> AdvisoryEvidence {
    record(
        id,
        vec![events(&[
            ("introduced", "0"),
            ("fixed", "1.2.5"),
            ("introduced", "2.0"),
            ("fixed", "2.0.3"),
        ])],
        &[],
    )
}

#[test]
fn a_single_fix_becomes_a_lower_bound() {
    let record = record(
        "CVE-1",
        vec![events(&[("introduced", "0"), ("fixed", "3.1.1")])],
        &[],
    );
    assert_eq!(
        planned(
            vec![finding(conda("openssl"), "3.1.0", vec![record.clone()])],
            &[]
        ),
        "openssl >=3.1.1"
    );
    assert_eq!(
        planned(vec![finding(pypi("Requests"), "3.1.0", vec![record])], &[]),
        "requests>=3.1.1"
    );
}

#[test]
fn release_branches_stay_separate() {
    // Conda can express both safe ranges; PEP 440 gets the highest one.
    assert_eq!(
        planned(
            vec![finding(
                conda("libfoo"),
                "2.0.1",
                vec![two_branches("CVE-1")]
            )],
            &[]
        ),
        "libfoo >=2.0.3"
    );
    assert_eq!(
        planned(
            vec![finding(
                conda("libfoo"),
                "1.2.0",
                vec![two_branches("CVE-1")]
            )],
            &[]
        ),
        "libfoo >=1.2.5,<2.0|>=2.0.3"
    );
    assert_eq!(
        planned(
            vec![finding(pypi("foo"), "2.0.1", vec![two_branches("CVE-1")])],
            &[]
        ),
        "foo>=2.0.3"
    );
}

#[test]
fn manifest_requirements_select_the_compatible_branch() {
    let findings = || vec![finding(pypi("foo"), "1.2.0", vec![two_branches("CVE-1")])];
    assert_eq!(
        planned(findings(), &[requirement(pypi("foo"), "<2")]),
        "foo>=1.2.5, <2.0"
    );
    assert_eq!(
        planned(
            vec![finding(
                conda("libfoo"),
                "1.2.0",
                vec![two_branches("CVE-1")]
            )],
            &[requirement(conda("libfoo"), "<2")]
        ),
        "libfoo >=1.2.5,<2.0|>=2.0.3"
    );
    assert_eq!(
        planned(findings(), &[requirement(pypi("foo"), ">=2.0,<2.0.3")]),
        "blocked: no safe release of foo satisfies the manifest requirement '>=2.0,<2.0.3'"
    );
    // Requirements without an exact interval form are left to the solver.
    assert_eq!(
        planned(findings(), &[requirement(pypi("foo"), "~=1.2")]),
        "foo>=2.0.3"
    );
}

#[test]
fn fixes_of_older_branches_are_not_downgrades() {
    let record = record(
        "CVE-1",
        vec![events(&[
            ("introduced", "0"),
            ("fixed", "1.9.5"),
            ("introduced", "2.0"),
        ])],
        &["not a version!"],
    );
    assert_eq!(
        planned(vec![finding(conda("libfoo"), "2.3", vec![record])], &[]),
        "blocked: libfoo 2.3 has no fixed release for CVE-1 in its own ecosystem"
    );
}

#[test]
fn findings_on_different_branches_keep_their_own_fix() {
    let record = || {
        record(
            "CVE-1",
            vec![events(&[
                ("introduced", "0"),
                ("fixed", "3.0.5"),
                ("introduced", "3.1.0"),
                ("fixed", "3.1.2"),
            ])],
            &[],
        )
    };
    assert_eq!(
        planned(
            vec![
                finding(conda("openssl"), "3.1.0", vec![record()]),
                finding(conda("openssl"), "3.0.4", vec![record()]),
            ],
            &[requirement(conda("openssl"), ">=3.0,<3.1")]
        ),
        "openssl >=3.0.5,<3.1.0|>=3.1.2"
    );
}

#[test]
fn records_are_intersected_and_affected_fixes_are_skipped() {
    // CVE-2 affects the release CVE-1 names as its fix, and lists 1.3 as
    // affected too. The later fix of CVE-2 remains admissible.
    let first = record(
        "CVE-1",
        vec![events(&[("introduced", "0"), ("fixed", "1.1")])],
        &[],
    );
    let second = record(
        "CVE-2",
        vec![events(&[("introduced", "1.1"), ("fixed", "1.2")])],
        &["1.3"],
    );
    assert_eq!(
        planned(
            vec![
                finding(conda("libfoo"), "1.0", vec![first.clone()]),
                finding(conda("libfoo"), "1.0", vec![second.clone()]),
            ],
            &[]
        ),
        "libfoo >=1.2,!=1.3"
    );
    assert_eq!(
        planned(vec![finding(pypi("foo"), "1.0", vec![first, second])], &[]),
        "foo>=1.2, !=1.3"
    );
}

#[test]
fn last_affected_excludes_the_last_affected_version() {
    let record = record(
        "CVE-1",
        vec![events(&[("introduced", "1.0"), ("last_affected", "1.4")])],
        &[],
    );
    assert_eq!(
        planned(vec![finding(pypi("foo"), "1.4", vec![record])], &[]),
        "foo>1.4"
    );
}

#[test]
fn recurrence_without_a_later_fix_has_no_safe_release() {
    let record = record(
        "CVE-1",
        vec![
            events(&[("introduced", "0"), ("fixed", "1.1")]),
            events(&[("introduced", "1.1")]),
        ],
        &[],
    );
    assert_eq!(
        planned(vec![finding(conda("libfoo"), "1.0", vec![record])], &[]),
        "blocked: every fixed release of libfoo reported by CVE-1 is affected by another of them"
    );
}

#[test]
fn versions_below_every_fix_are_not_remediation() {
    // Versions before `introduced` are unaffected, but no fix documents them.
    let record = record(
        "CVE-1",
        vec![events(&[("introduced", "2.0"), ("fixed", "2.5")])],
        &[],
    );
    assert_eq!(
        planned(
            vec![finding(conda("libfoo"), "2.1", vec![record])],
            &[requirement(conda("libfoo"), "<2")]
        ),
        "blocked: no safe release of libfoo satisfies the manifest requirement '<2'"
    );
}

#[test]
fn every_failing_match_needs_a_fix_in_its_own_ecosystem() {
    let mut elsewhere = finding(conda("libfoo"), "1.0", vec![]);
    elsewhere.id = "CVE-9".to_string();
    elsewhere.upstream_releases = vec![crate::UpstreamRelease {
        version: "1.1".to_string(),
        conda_forge_build: crate::CondaForgeBuild::Missing,
    }];
    let fixable = finding(
        pypi("foo"),
        "1.0",
        vec![record(
            "CVE-1",
            vec![events(&[("introduced", "0"), ("fixed", "1.1")])],
            &[],
        )],
    );
    // One blocked package blocks the whole plan.
    assert_eq!(
        planned(vec![elsewhere, fixable], &[]),
        "blocked: libfoo 1.0 has no fixed release for CVE-9 in its own ecosystem"
    );
}

#[test]
fn unbounded_matches_and_unparsable_fixes_block() {
    let mut unbounded = finding(
        conda("libfoo"),
        "1.0",
        vec![record(
            "CVE-1",
            vec![events(&[("introduced", "0"), ("fixed", "1.1")])],
            &[],
        )],
    );
    unbounded.match_confidence = MatchConfidence::VersionUnbounded;
    assert_eq!(
        planned(vec![unbounded], &[]),
        "blocked: CVE-1 affects every version of libfoo, so no constraint can avoid it"
    );

    let garbled = finding(
        pypi("foo"),
        "1.0",
        vec![record(
            "CVE-2",
            vec![events(&[("introduced", "0"), ("fixed", "not a version!")])],
            &[],
        )],
    );
    assert_eq!(
        planned(vec![garbled], &[]),
        "blocked: the fix data of CVE-2 for foo cannot be interpreted: 'not a version!' is not a valid version"
    );
}

#[test]
fn only_policy_failing_matches_are_planned() {
    let mut warning = finding(
        pypi("foo"),
        "1.0",
        vec![record(
            "CVE-1",
            vec![events(&[("introduced", "0"), ("fixed", "1.1")])],
            &[],
        )],
    );
    warning.severity = SeverityRating::Low;
    let mut ignored = warning.clone();
    ignored.package.name = pypi("bar");
    ignored.ignore_mode = Some(IgnoreMode::Permanent);
    let audit = AuditReport {
        matches: vec![warning],
        ignored: vec![ignored],
        ..AuditReport::default()
    };
    let policy = Policy {
        fail_on: Some(SeverityRating::High),
        ..Policy::default()
    };
    assert!(plan(&audit, &policy, &[]).unwrap().is_empty());
}
