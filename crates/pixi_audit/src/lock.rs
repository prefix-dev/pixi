//! Collecting auditable packages straight from a `pixi.lock` file.

use std::collections::{BTreeMap, BTreeSet};

use rattler_lock::{LockFile, LockedPackage};
use thiserror::Error;

use crate::{
    audit::{audit, is_auditable},
    client::{AuditError, BasiliskClient, OsvDevClient},
    report::{
        AuditPackage, AuditReport, IgnoreRule, InvalidPackageName, PackageEcosystem, PackageName,
    },
    threat::ThreatSources,
};

#[derive(Debug, Error)]
pub enum LockFileAuditError {
    #[error("the lock file contains no environments")]
    EmptyLockFile,
    /// A filter named an environment the lock file does not contain. This is
    /// an error rather than an empty selection: a misspelled filter must not
    /// silently produce a clean-looking audit.
    #[error("the lock file contains no environment named '{name}'")]
    UnknownEnvironment { name: String },
    #[error("the selected environments contain no platform named '{name}'")]
    UnknownPlatform { name: String },
    #[error(transparent)]
    InvalidPackageName(#[from] InvalidPackageName),
    #[error(transparent)]
    Audit(#[from] AuditError),
}

fn is_selected(filter: &[String], name: &str) -> bool {
    filter.is_empty() || filter.iter().any(|candidate| candidate == name)
}

/// One locked package in one environment and platform.
struct LockedEntry {
    environment: String,
    platform: String,
    name: PackageName,
    version: Option<String>,
    ecosystem: PackageEcosystem,
}

/// Whether a PyPI artifact comes from the public index. Only wheels from
/// `files.pythonhosted.org` qualify: the name of a package from a mirror, a
/// private index, Git, or a path does not establish its public identity.
fn is_public_pypi_wheel(pypi: &rattler_lock::PypiPackageData) -> bool {
    matches!(
        pypi.location().inner().as_url(),
        Some(url)
            if pypi.as_wheel().is_some()
                && url.scheme() == "https"
                && url.host_str() == Some("files.pythonhosted.org")
                && url.port_or_known_default() == Some(443)
                && url.path().starts_with("/packages/")
    )
}

fn locked_entries(lock_file: &LockFile) -> Result<Vec<LockedEntry>, InvalidPackageName> {
    let mut entries = Vec::new();
    for (environment_name, environment) in lock_file.environments() {
        for (platform, packages) in environment.packages_by_platform() {
            for package in packages {
                let (name, version, ecosystem) = match package {
                    LockedPackage::Conda(conda) => {
                        let channel = conda
                            .as_binary()
                            .and_then(|binary| binary.channel.as_ref())
                            .map(|channel| channel.to_string());
                        (
                            PackageName::Conda(conda.name().clone()),
                            conda.record().map(|record| record.version.to_string()),
                            PackageEcosystem::from_channel_url(channel.as_deref()),
                        )
                    }
                    LockedPackage::Pypi(pypi) => (
                        PackageName::pypi(pypi.name().as_ref())?,
                        pypi.version().map(ToString::to_string),
                        if is_public_pypi_wheel(pypi) {
                            PackageEcosystem::Pypi
                        } else {
                            PackageEcosystem::Other("non-PyPI source".to_string())
                        },
                    ),
                };
                entries.push(LockedEntry {
                    environment: environment_name.to_string(),
                    platform: platform.name().to_string(),
                    name,
                    version,
                    ecosystem,
                });
            }
        }
    }
    Ok(entries)
}

/// Collects deduplicated packages to audit from the lock file, together with
/// the set of environments (across all selected platforms) containing each.
/// Empty filters select every environment/platform.
pub fn collect_packages(
    lock_file: &LockFile,
    environment_filter: &[String],
    platform_filter: &[String],
) -> Result<Vec<AuditPackage>, LockFileAuditError> {
    if lock_file.environments().len() == 0 {
        return Err(LockFileAuditError::EmptyLockFile);
    }

    // Reject filters that match nothing, so a typo cannot silently shrink
    // the audit to an empty, clean-looking report.
    for name in environment_filter {
        if !lock_file.environments().any(|(env, _)| env == name) {
            return Err(LockFileAuditError::UnknownEnvironment { name: name.clone() });
        }
    }
    for name in platform_filter {
        let known = lock_file
            .environments()
            .any(|(environment_name, environment)| {
                is_selected(environment_filter, environment_name)
                    && environment
                        .packages_by_platform()
                        .any(|(platform, _)| platform.name().as_str() == name.as_str())
            });
        if !known {
            return Err(LockFileAuditError::UnknownPlatform { name: name.clone() });
        }
    }

    // Preserve ecosystem identity even when a channel name matches an OSV ecosystem.
    type DedupKey = (PackageName, Option<String>, PackageEcosystem);
    let mut dedup: BTreeMap<DedupKey, BTreeSet<String>> = BTreeMap::new();
    for entry in locked_entries(lock_file)? {
        if !is_selected(environment_filter, &entry.environment)
            || !is_selected(platform_filter, &entry.platform)
        {
            continue;
        }
        dedup
            .entry((entry.name, entry.version, entry.ecosystem))
            .or_default()
            .insert(entry.environment);
    }

    Ok(dedup
        .into_iter()
        .map(|((name, version, ecosystem), environments)| AuditPackage {
            name,
            version,
            ecosystem,
            environments: environments.into_iter().collect(),
        })
        .collect())
}

/// Identifies a locked package by where it is locked, independent of its
/// version and source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoverageKey {
    pub environment: String,
    pub platform: String,
    pub name: PackageName,
}

/// Whether a locked package is covered by a vulnerability database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageStatus {
    Audited,
    NotAudited { source: PackageEcosystem },
}

/// The audit coverage of every package in a lock file, per environment and
/// platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage(BTreeMap<CoverageKey, CoverageStatus>);

/// How a candidate lock file lost audit coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageRegression {
    /// A package the original lock file does not contain is not audited.
    NewlyUnaudited {
        key: CoverageKey,
        source: PackageEcosystem,
    },
    /// An audited package is no longer audited, for example because it now
    /// comes from an unsupported source.
    LostCoverage {
        key: CoverageKey,
        source: PackageEcosystem,
    },
}

impl Coverage {
    pub fn from_lock_file(lock_file: &LockFile) -> Result<Self, InvalidPackageName> {
        Ok(Self(
            locked_entries(lock_file)?
                .into_iter()
                .map(|entry| {
                    let package = AuditPackage {
                        name: entry.name.clone(),
                        version: entry.version,
                        ecosystem: entry.ecosystem,
                        environments: Vec::new(),
                    };
                    let status = if is_auditable(&package) {
                        CoverageStatus::Audited
                    } else {
                        CoverageStatus::NotAudited {
                            source: package.ecosystem,
                        }
                    };
                    let key = CoverageKey {
                        environment: entry.environment,
                        platform: entry.platform,
                        name: entry.name,
                    };
                    (key, status)
                })
                .collect(),
        ))
    }

    /// Where `candidate` audits less than `self`. A package missing from the
    /// candidate was removed, which is not a loss of coverage; a package that
    /// was not audited before and still is not is a pre-existing gap.
    pub fn regressions(&self, candidate: &Coverage) -> Vec<CoverageRegression> {
        candidate
            .0
            .iter()
            .filter_map(|(key, status)| {
                let CoverageStatus::NotAudited { source } = status else {
                    return None;
                };
                match self.0.get(key) {
                    None => Some(CoverageRegression::NewlyUnaudited {
                        key: key.clone(),
                        source: source.clone(),
                    }),
                    Some(CoverageStatus::Audited) => Some(CoverageRegression::LostCoverage {
                        key: key.clone(),
                        source: source.clone(),
                    }),
                    Some(CoverageStatus::NotAudited { .. }) => None,
                }
            })
            .collect()
    }
}

/// Audits every package in the lock file (subject to the environment and
/// platform filters) against the vulnerability database, attaching threat
/// data from `threat` to the vulnerability matches.
pub async fn audit_lock_file(
    client: &BasiliskClient,
    osv_client: &OsvDevClient,
    lock_file: &LockFile,
    environment_filter: &[String],
    platform_filter: &[String],
    ignore: &[IgnoreRule],
    threat: ThreatSources<'_>,
) -> Result<AuditReport, LockFileAuditError> {
    let packages = collect_packages(lock_file, environment_filter, platform_filter)?;
    Ok(audit(client, osv_client, packages, ignore, threat).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{basilisk_client, osv_client as test_osv_client, spawn_router};

    const FIXTURE: &str = r#"version: 6
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/openssl-3.1.0-hd590300_3.conda
      - conda: https://conda.anaconda.org/bioconda/linux-64/samtools-1.19-h50ea8bc_0.conda
      - pypi: https://files.pythonhosted.org/packages/py3/r/requests/requests-2.32.0-py3-none-any.whl
      osx-arm64:
      - conda: https://conda.anaconda.org/conda-forge/osx-arm64/openssl-3.1.0-h53f4e23_3.conda
  prod:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/openssl-3.1.0-hd590300_3.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/linux-64/openssl-3.1.0-hd590300_3.conda
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  md5: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  license: Apache-2.0
  size: 100
  timestamp: 1700000000000
- conda: https://conda.anaconda.org/conda-forge/osx-arm64/openssl-3.1.0-h53f4e23_3.conda
  sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  md5: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  license: Apache-2.0
  size: 100
  timestamp: 1700000000000
- conda: https://conda.anaconda.org/bioconda/linux-64/samtools-1.19-h50ea8bc_0.conda
  sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
  md5: cccccccccccccccccccccccccccccccc
  license: MIT
  size: 100
  timestamp: 1700000000000
- pypi: https://files.pythonhosted.org/packages/py3/r/requests/requests-2.32.0-py3-none-any.whl
  name: requests
  version: 2.32.0
  sha256: dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd
  requires_python: '>=3.8'
"#;

    fn fixture() -> LockFile {
        LockFile::from_str_with_base_directory(FIXTURE, None).expect("fixture parses")
    }

    #[test]
    fn collects_deduplicated_packages_with_environments() {
        let packages = collect_packages(&fixture(), &[], &[]).unwrap();

        let openssl: Vec<&AuditPackage> = packages
            .iter()
            .filter(|p| p.name.as_str() == "openssl")
            .collect();
        assert_eq!(openssl.len(), 1);
        assert_eq!(openssl[0].version.as_deref(), Some("3.1.0"));
        assert_eq!(openssl[0].ecosystem, PackageEcosystem::CondaForge);
        assert_eq!(openssl[0].environments, vec!["default", "prod"]);

        let samtools = packages
            .iter()
            .find(|p| p.name.as_str() == "samtools")
            .unwrap();
        assert_eq!(
            samtools.ecosystem,
            PackageEcosystem::Other("bioconda".to_string())
        );
        assert_eq!(samtools.environments, vec!["default"]);

        let requests = packages
            .iter()
            .find(|p| p.name.as_str() == "requests")
            .unwrap();
        assert_eq!(requests.ecosystem, PackageEcosystem::Pypi);
        assert_eq!(requests.version.as_deref(), Some("2.32.0"));

        assert_eq!(packages.len(), 3);
    }

    #[test]
    fn non_public_python_sources_are_not_pypi() {
        let public_url = "https://files.pythonhosted.org/packages/py3/r/requests/requests-2.32.0-py3-none-any.whl";
        for source in [
            "https://private.example/requests-2.32.0-py3-none-any.whl",
            "https://files.pythonhosted.org.evil.example/packages/requests.whl",
            "http://files.pythonhosted.org/packages/requests.whl",
            "https://files.pythonhosted.org:8443/packages/requests.whl",
            "git+https://github.com/example/requests.git#1234567890123456789012345678901234567890",
            "./requests",
        ] {
            let contents = FIXTURE.replace(public_url, source);
            let lock = LockFile::from_str_with_base_directory(&contents, None).unwrap();
            let packages = collect_packages(&lock, &[], &[]).unwrap();
            let requests = packages
                .iter()
                .find(|package| package.name.as_str() == "requests")
                .unwrap();
            assert_eq!(
                requests.ecosystem,
                PackageEcosystem::Other("non-PyPI source".to_string()),
                "{source}"
            );
        }
    }

    #[test]
    fn environment_filter_limits_collection() {
        let packages = collect_packages(&fixture(), &["prod".to_string()], &[]).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name.as_str(), "openssl");
        assert_eq!(packages[0].environments, vec!["prod"]);
    }

    #[test]
    fn platform_filter_limits_collection() {
        let packages = collect_packages(&fixture(), &[], &["osx-arm64".to_string()]).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name.as_str(), "openssl");
        assert_eq!(packages[0].environments, vec!["default"]);
    }

    #[test]
    fn unknown_environment_filter_is_an_error() {
        let err = collect_packages(&fixture(), &["pord".to_string()], &[]).unwrap_err();
        assert!(
            matches!(&err, LockFileAuditError::UnknownEnvironment { name } if name == "pord"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_platform_filter_is_an_error() {
        let err = collect_packages(&fixture(), &[], &["linux-46".to_string()]).unwrap_err();
        assert!(
            matches!(&err, LockFileAuditError::UnknownPlatform { name } if name == "linux-46"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn environment_and_platform_filters_must_intersect() {
        let err = collect_packages(
            &fixture(),
            &["prod".to_string()],
            &["osx-arm64".to_string()],
        )
        .unwrap_err();
        assert!(
            matches!(&err, LockFileAuditError::UnknownPlatform { name } if name == "osx-arm64"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn coverage_regressions_compare_each_environment_and_platform() {
        let baseline = Coverage::from_lock_file(&fixture()).unwrap();
        assert!(baseline.regressions(&baseline).is_empty());

        // Removing a dependency is not a loss of coverage, and samtools was
        // never audited.
        let without_requests = FIXTURE.replace(
            "      - pypi: https://files.pythonhosted.org/packages/py3/r/requests/requests-2.32.0-py3-none-any.whl\n      osx-arm64:",
            "      osx-arm64:",
        );
        let candidate = LockFile::from_str_with_base_directory(&without_requests, None).unwrap();
        assert!(
            baseline
                .regressions(&Coverage::from_lock_file(&candidate).unwrap())
                .is_empty()
        );

        // An audited package moving to a private source loses coverage.
        let private = FIXTURE.replace(
            "https://files.pythonhosted.org/packages/py3/r/requests/requests-2.32.0-py3-none-any.whl",
            "https://private.example/requests-2.32.0-py3-none-any.whl",
        );
        let candidate = LockFile::from_str_with_base_directory(&private, None).unwrap();
        let regressions = baseline.regressions(&Coverage::from_lock_file(&candidate).unwrap());
        assert!(
            matches!(
                regressions.as_slice(),
                [CoverageRegression::LostCoverage { key, .. }]
                    if key.name.as_str() == "requests" && key.environment == "default"
            ),
            "{regressions:?}"
        );

        // A new unaudited package, even one audited elsewhere in the lock
        // file, is a regression for its environment.
        let spread = FIXTURE.replace(
            "  prod:\n    channels:\n    - url: https://conda.anaconda.org/conda-forge/\n    packages:\n      linux-64:\n",
            "  prod:\n    channels:\n    - url: https://conda.anaconda.org/conda-forge/\n    packages:\n      linux-64:\n      - conda: https://conda.anaconda.org/bioconda/linux-64/samtools-1.19-h50ea8bc_0.conda\n",
        );
        let candidate = LockFile::from_str_with_base_directory(&spread, None).unwrap();
        let regressions = baseline.regressions(&Coverage::from_lock_file(&candidate).unwrap());
        assert!(
            matches!(
                regressions.as_slice(),
                [CoverageRegression::NewlyUnaudited { key, .. }]
                    if key.name.as_str() == "samtools" && key.environment == "prod"
            ),
            "{regressions:?}"
        );
    }

    #[test]
    fn empty_lock_file_is_an_error() {
        let empty = LockFile::default();
        let err = collect_packages(&empty, &[], &[]).unwrap_err();
        assert!(matches!(err, LockFileAuditError::EmptyLockFile));
    }

    #[tokio::test]
    async fn audits_a_lock_file_end_to_end() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };

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
            .route(
                "/v1/vulns/{id}",
                get(|| async {
                    Json(serde_json::json!({
                        "id": "BSLK-1",
                        "summary": "Buffer overflow in openssl",
                        "severity": [{
                            "type": "CVSS_V3",
                            "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"
                        }],
                        "affected": [{
                            "package": {"ecosystem": "conda-forge", "name": "openssl"},
                            "versions": ["3.1.0"]
                        }]
                    }))
                }),
            );
        let base = spawn_router(app).await;
        let client = basilisk_client(base.clone());
        let osv_client = test_osv_client(base);

        let report = audit_lock_file(
            &client,
            &osv_client,
            &fixture(),
            &[],
            &[],
            &[],
            ThreatSources::default(),
        )
        .await
        .unwrap();

        assert_eq!(report.matches.len(), 1);
        let vuln_match = &report.matches[0];
        assert_eq!(vuln_match.package.name.as_str(), "openssl");
        assert_eq!(vuln_match.id, "BSLK-1");
        assert_eq!(vuln_match.severity, crate::report::SeverityRating::Critical);
        assert_eq!(vuln_match.environments, vec!["default", "prod"]);

        // samtools (bioconda) is not audited; requests is audited through OSV.
        assert_eq!(report.summary.not_audited, 1);
        assert_eq!(report.summary.audited, 2);
    }
}
