use super::*;

#[test]
fn package_versions_use_native_ordering() {
    for (ecosystem, lower, higher) in [
        (PackageEcosystem::CondaForge, "1.9", "1.10"),
        (PackageEcosystem::CondaForge, "1.0dev1", "1.0"),
        (PackageEcosystem::Pypi, "1.0rc1", "1.0"),
        (PackageEcosystem::Pypi, "1.0", "1.0.post1"),
        (PackageEcosystem::Pypi, "9.0", "1!1.0"),
    ] {
        let lower = PackageVersion::parse(&ecosystem, lower).unwrap();
        let higher = PackageVersion::parse(&ecosystem, higher).unwrap();
        assert_eq!(lower.partial_cmp(&higher), Some(Ordering::Less));
        assert_eq!(higher.partial_cmp(&lower), Some(Ordering::Greater));
    }
    for ecosystem in [PackageEcosystem::CondaForge, PackageEcosystem::Pypi] {
        let a = PackageVersion::parse(&ecosystem, "1.0").unwrap();
        let b = PackageVersion::parse(&ecosystem, "1.0.0").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Equal));
    }
}

#[test]
fn package_versions_from_different_ecosystems_are_incomparable() {
    let conda = PackageVersion::parse(&PackageEcosystem::CondaForge, "1.0").unwrap();
    let pypi = PackageVersion::parse(&PackageEcosystem::Pypi, "1.0").unwrap();
    assert_ne!(conda, pypi);
    assert_eq!(conda.partial_cmp(&pypi), None);
    assert_eq!(pypi.partial_cmp(&conda), None);
}

#[test]
fn package_version_parsing_rejects_invalid_and_unsupported_inputs() {
    for ecosystem in [PackageEcosystem::CondaForge, PackageEcosystem::Pypi] {
        assert!(matches!(
            PackageVersion::parse(&ecosystem, "not a version"),
            Err(ParsePackageVersionError::InvalidVersion { .. })
        ));
    }
    assert!(matches!(
        PackageVersion::parse(&PackageEcosystem::Pypi, "1.0foo"),
        Err(ParsePackageVersionError::InvalidVersion { .. })
    ));
    assert!(matches!(
        PackageVersion::parse(&PackageEcosystem::Other("npm".into()), "1.0"),
        Err(ParsePackageVersionError::UnsupportedEcosystem(_))
    ));
}

#[test]
fn ignore_scope_validates_versions_for_the_selected_ecosystem() {
    for (ecosystem, version, valid) in [
        (None, "1.0foo", true),
        (Some("Conda-Forge"), "1.0foo", true),
        (Some("PyPI"), "1.0foo", false),
        (Some("pYpI"), "1.0rc1", true),
        (None, "not a version", false),
        (Some("conda-forge"), "not a version", false),
        (Some("PyPI"), "not a version", false),
        (Some("npm"), "1.0", false),
    ] {
        let mut scope = IgnoreScope {
            ecosystem: ecosystem.map(str::to_string),
            version: Some(version.to_string()),
            ..IgnoreScope::default()
        };
        assert_eq!(scope.validate().is_ok(), valid, "{scope:?}");
        scope.up_to_version = scope.version.take();
        assert_eq!(scope.validate().is_ok(), valid, "{scope:?}");
    }
}

fn scope_matches(scope: &IgnoreScope, package: &PackageId) -> bool {
    let locked = package.parsed_version().ok();
    ScopeMatcher::parse(scope)
        .unwrap()
        .matches(package, locked.as_ref())
}

#[test]
fn unqualified_scope_compares_under_the_package_ecosystem() {
    // Not the first version parser that accepts the bound: `1.0foo` is a
    // conda version but not a PEP 440 one.
    let mut package = PackageId {
        name: PackageName::conda("openssl").unwrap(),
        version: "1.0foo".into(),
        ecosystem: PackageEcosystem::CondaForge,
    };
    let mut scope = IgnoreScope {
        version: Some("1.0foo".into()),
        up_to_version: Some("1.0foo".into()),
        ..IgnoreScope::default()
    };
    assert!(scope_matches(&scope, &package));
    package.ecosystem = PackageEcosystem::Pypi;
    assert!(!scope_matches(&scope, &package));
    package.version = "1.0".into();
    scope.version = Some("1.0.0".into());
    scope.up_to_version = Some("1.0rc1".into());
    assert!(!scope_matches(&scope, &package));
    scope.up_to_version = Some("1.0".into());
    assert!(scope_matches(&scope, &package));
    package.ecosystem = PackageEcosystem::Other("custom".into());
    assert!(!scope_matches(&scope, &package));
    package.version = "not a version".into();
    assert!(scope_matches(&IgnoreScope::default(), &package));
}

#[test]
fn severity_band_round_trips_through_its_name() {
    for rating in SeverityRating::ALL {
        assert_eq!(rating.name().parse::<SeverityRating>(), Ok(rating));
        assert_eq!(rating.to_string(), rating.name());
    }
    assert!("HIGH".parse::<SeverityRating>().is_err());
    assert!("severe".parse::<SeverityRating>().is_err());
}

#[test]
fn package_names_normalize_per_ecosystem() {
    let pypi = PackageName::pypi("Ruamel.YAML").unwrap();
    assert_eq!(pypi.as_str(), "ruamel-yaml");
    assert_eq!(pypi, PackageName::pypi("ruamel__yaml").unwrap());
    assert!(pypi.matches("ruamel.yaml"));
    assert!(!pypi.matches("ruamel"));

    let conda = PackageName::conda("OpenSSL").unwrap();
    assert_eq!(conda.as_str(), "openssl");
    assert!(conda.matches("openssl"));
    assert!(!conda.matches("open-ssl"));
    assert!(PackageName::conda("not valid").is_err());

    assert_eq!(
        serde_json::to_value([&pypi, &conda]).unwrap(),
        serde_json::json!(["ruamel-yaml", "openssl"])
    );
}

#[test]
fn ecosystem_serializes_as_osv_string() {
    let json = serde_json::to_value([
        PackageEcosystem::CondaForge,
        PackageEcosystem::Pypi,
        PackageEcosystem::Other("bioconda".to_string()),
    ])
    .unwrap();
    assert_eq!(json, serde_json::json!(["conda-forge", "PyPI", "bioconda"]));
}

fn severity(kind: &str, score: &str) -> crate::types::OsvSeverity {
    crate::types::OsvSeverity {
        kind: kind.to_string(),
        score: score.to_string(),
        source: None,
    }
}

#[test]
fn severity_band_from_cvss_vector() {
    // CVSS:3.1 9.8 => critical
    assert_eq!(
        SeverityRating::from_severities(&[severity(
            "CVSS_V3",
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"
        )]),
        SeverityRating::Critical
    );
    // Unparsable => unknown
    assert_eq!(
        SeverityRating::from_severities(&[]),
        SeverityRating::Unknown
    );
}

#[test]
fn severity_band_accepts_nvd_style_type_labels() {
    // basilisk's NVD ingestion labels entries "CVSS_V3.1" / "CVSS_V3.0".
    assert_eq!(
        SeverityRating::from_severities(&[severity(
            "CVSS_V3.1",
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:N/A:N"
        )]),
        SeverityRating::High
    );
}

#[test]
fn severity_band_from_cvss_v4() {
    // CVSS:4.0 9.3 => critical
    assert_eq!(
        SeverityRating::from_severities(&[severity(
            "CVSS_V4",
            "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"
        )]),
        SeverityRating::Critical
    );
}

#[test]
fn severity_band_from_cvss_v2() {
    // Known NVD base scores: 7.5 => high, 4.3 => medium.
    assert_eq!(
        SeverityRating::from_severities(&[severity("CVSS_V2.0", "AV:N/AC:L/Au:N/C:P/I:P/A:P")]),
        SeverityRating::High
    );
    assert_eq!(
        SeverityRating::from_severities(&[severity("CVSS_V2.0", "AV:N/AC:M/Au:N/C:P/I:N/A:N")]),
        SeverityRating::Medium
    );
    assert_eq!(
        SeverityRating::from_severities(&[severity("CVSS_V2.0", "AV:L/AC:H/Au:N/C:N/I:P/A:N")]),
        SeverityRating::Low
    );
}

#[test]
fn cvss_v2_has_no_critical_band() {
    // v2's qualitative scale tops out at "high" (7.0-10.0); a 10.0 v2-only
    // advisory must not be promoted to critical.
    assert_eq!(
        SeverityRating::from_severities(&[severity("CVSS_V2.0", "AV:N/AC:L/Au:N/C:C/I:C/A:C")]),
        SeverityRating::High
    );
}

#[test]
fn highest_band_across_mixed_severity_entries_wins() {
    assert_eq!(
        SeverityRating::from_severities(&[
            severity("CVSS_V2.0", "AV:N/AC:M/Au:N/C:P/I:N/A:N"), // medium
            severity("CVSS_V3.1", "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), // critical
        ]),
        SeverityRating::Critical
    );
}

#[test]
fn classify_channel_urls() {
    assert_eq!(
        PackageEcosystem::from_channel_url(Some("https://conda.anaconda.org/conda-forge/")),
        PackageEcosystem::CondaForge
    );
    assert_eq!(
        PackageEcosystem::from_channel_url(Some("https://prefix.dev/conda-forge")),
        PackageEcosystem::CondaForge
    );
    assert_eq!(
        PackageEcosystem::from_channel_url(Some("https://conda.anaconda.org/bioconda/")),
        PackageEcosystem::Other("bioconda".to_string())
    );
    assert_eq!(
        PackageEcosystem::from_channel_url(None),
        PackageEcosystem::Other("unknown".to_string())
    );
}

#[test]
fn label_channels_belong_to_their_channel() {
    assert_eq!(
        PackageEcosystem::from_channel_url(Some(
            "https://conda.anaconda.org/conda-forge/label/rust_dev/"
        )),
        PackageEcosystem::CondaForge
    );
    assert_eq!(
        PackageEcosystem::from_channel_url(Some("https://prefix.dev/conda-forge/label/broken")),
        PackageEcosystem::CondaForge
    );
    assert_eq!(
        PackageEcosystem::from_channel_url(Some("https://conda.anaconda.org/bioconda/label/main")),
        PackageEcosystem::Other("bioconda".to_string())
    );
}

#[test]
fn unrelated_origins_are_not_conda_forge() {
    for channel in [
        "https://private.example/conda-forge",
        "https://conda.anaconda.org.evil.example/conda-forge",
        "https://prefix.dev/private/conda-forge",
        "http://conda.anaconda.org/conda-forge",
        "https://conda.anaconda.org:8443/conda-forge/label/main",
    ] {
        assert_eq!(
            PackageEcosystem::from_channel_url(Some(channel)),
            PackageEcosystem::Other("conda-forge".to_string()),
            "{channel}"
        );
    }
}
