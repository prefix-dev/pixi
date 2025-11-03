use std::collections::{BTreeMap, BTreeSet, HashMap};

use pixi_spec::SourceSpec;
use rattler_conda_types::{
    MatchSpec, Matches, NoArchType, PackageName, PackageRecord, PackageUrl, VersionWithSource,
    package::RunExportsJson,
};
use rattler_digest::{Md5Hash, Sha256, Sha256Hash};
use rattler_lock::{CondaPackageData, CondaSourceData};
use std::str::FromStr;

use pixi_git::sha::GitSha;
use pixi_spec::GitReference;
use rattler_lock::{GitShallowSpec, PackageBuildSource};
use serde::{Deserialize, Serialize};

use crate::SelectedVariant;
use crate::{ParseLockFileError, PinnedGitCheckout, PinnedSourceSpec};

/// A minimal record of a source package stored in the lock file.
///
/// This contains only the essential information needed to identify and locate
/// a source package (name, source location, variants, dependencies). Notably,
/// it does **not** include version or build information, which is only known
/// after the package has been built or its metadata has been resolved.
///
/// This minimal representation is sufficient to perform an install (build the
/// package from source), but not to perform a solve (which requires version
/// and build information).
///
/// For the complete package information with version/build details, see
/// [`SourcePackageRecord`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecord {
    /// The name of the package
    pub name: PackageName,

    /// Exact definition of the source of the package.
    pub manifest_source: PinnedSourceSpec,

    /// The optional pinned source where the build should be executed
    /// This is used when the manifest is not in the same location ad
    pub build_source: Option<PinnedSourceSpec>,

    /// Conda-build variants used to disambiguate between multiple source packages
    /// at the same location.
    pub variants: SelectedVariant,

    /// Optionally the version of the package
    pub version: Option<VersionWithSource>,

    /// Specification of packages this package depends on
    pub depends: Vec<String>,

    /// Additional constraints on packages
    pub constrains: Vec<String>,

    /// Experimental: additional dependencies grouped by feature name
    pub experimental_extra_depends: BTreeMap<String, Vec<String>>,

    /// The specific license of the package
    pub license: Option<String>,

    /// Package identifiers of packages that are equivalent to this package but
    /// from other ecosystems (e.g., PyPI)
    pub purls: Option<BTreeSet<PackageUrl>>,

    /// The hash of the input that was used to build the metadata of the
    /// package. This can be used to verify that the metadata is still valid.
    ///
    /// If this is `None`, the input hash was not computed or is not relevant
    /// for this record. The record can always be considered up to date.
    pub input_hash: Option<InputHash>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceSpec>,

    /// Python site-packages path if this is a Python package
    pub python_site_packages_path: Option<String>,
}

/// A complete source package record with full metadata after resolution or building.
///
/// This extends [`SourceRecord`] with complete package metadata including version,
/// build string, build number, timestamp, and hashes. This information is obtained
/// after building the package or resolving its metadata from the recipe.
///
/// Unlike [`SourceRecord`], this contains all the information needed to perform
/// dependency solving. It is **only** used during solve operations; for install
/// operations, [`SourceRecord`] contains sufficient information.
///
/// This is **not** stored in the lock file (only the minimal [`SourceRecord`] is persisted).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourcePackageRecord {
    /// The base source record
    pub source_record: SourceRecord,

    /// The version of the package
    pub version: VersionWithSource,

    /// The build string of the package
    pub build: String,

    /// The build number of the package
    pub build_number: rattler_conda_types::BuildNumber,

    /// The subdir of the package
    pub subdir: String,

    /// Optionally the architecture the package supports
    pub arch: Option<String>,

    /// Optionally the platform the package supports
    pub platform: Option<String>,

    /// MD5 hash of the package archive
    pub md5: Option<Md5Hash>,

    /// SHA256 hash of the package archive
    pub sha256: Option<Sha256Hash>,

    /// Size of the package archive in bytes
    pub size: Option<u64>,

    /// Track features
    pub track_features: Vec<String>,

    /// Features (deprecated)
    pub features: Option<String>,

    /// License family
    pub license_family: Option<String>,

    /// Timestamp of when the package was built
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Run exports information
    pub run_exports: Option<RunExportsJson>,

    /// NoArch type
    pub noarch: NoArchType,

    /// Legacy bz2 MD5 hash
    pub legacy_bz2_md5: Option<Md5Hash>,

    /// Legacy bz2 size
    pub legacy_bz2_size: Option<u64>,
}

impl From<SourcePackageRecord> for SourceRecord {
    fn from(value: SourcePackageRecord) -> Self {
        value.source_record
    }
}

impl From<SourcePackageRecord> for PackageRecord {
    fn from(value: SourcePackageRecord) -> Self {
        PackageRecord {
            name: value.source_record.name,
            version: value.version,
            build: value.build,
            build_number: value.build_number,
            subdir: value.subdir,
            depends: value.source_record.depends,
            constrains: value.source_record.constrains,
            arch: value.arch,
            platform: value.platform,
            md5: value.md5,
            sha256: value.sha256,
            size: value.size,
            license: value.source_record.license,
            license_family: value.license_family,
            purls: value.source_record.purls,
            track_features: value.track_features,
            features: value.features,
            timestamp: value.timestamp.map(From::from),
            run_exports: value.run_exports,
            experimental_extra_depends: value.source_record.experimental_extra_depends,
            noarch: value.noarch,
            legacy_bz2_md5: value.legacy_bz2_md5,
            legacy_bz2_size: value.legacy_bz2_size,
            python_site_packages_path: value.source_record.python_site_packages_path,
        }
    }
}

impl SourcePackageRecord {
    /// Convert to a PackageRecord
    pub fn package_record(&self) -> PackageRecord {
        self.clone().into()
    }
}

/// Defines the hash of the input files that were used to build the metadata of
/// the record. If reevaluating and hashing the globs results in a different
/// hash, the metadata is considered invalid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputHash {
    /// The hash of the input files that matched the globs.
    #[serde(
        serialize_with = "rattler_digest::serde::serialize::<_, Sha256>",
        deserialize_with = "rattler_digest::serde::deserialize::<_, Sha256>"
    )]
    pub hash: Sha256Hash,

    /// The globs that were used to compute the hash.
    pub globs: BTreeSet<String>,
}

impl From<SourceRecord> for CondaPackageData {
    fn from(value: SourceRecord) -> Self {
        CondaPackageData::Source(CondaSourceData {
            name: value.name,
            location: value.manifest_source.into(),
            variants: value.variants,
            version: value.version,
            depends: value.depends,
            constrains: value.constrains,
            experimental_extra_depends: value.experimental_extra_depends,
            license: value.license,
            purls: value.purls,
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            input: value.input_hash.map(|i| rattler_lock::InputHash {
                hash: i.hash,
                globs: Vec::from_iter(i.globs),
            }),
            package_build_source: None,
            python_site_packages_path: value.python_site_packages_path,
            dev: false, // TODO: adapt this when we implement the dev feature
        })
    }
}

impl TryFrom<CondaSourceData> for SourceRecord {
    type Error = ParseLockFileError;

    fn try_from(value: CondaSourceData) -> Result<Self, Self::Error> {
        let pinned_source_spec = value.package_build_source.map(|source| match source {
            PackageBuildSource::Git {
                url,
                spec,
                rev,
                subdir,
            } => {
                let reference = match spec {
                    Some(GitShallowSpec::Branch(branch)) => GitReference::Branch(branch),
                    Some(GitShallowSpec::Tag(tag)) => GitReference::Tag(tag),
                    Some(GitShallowSpec::Rev) => GitReference::Rev(rev.clone()),
                    None => GitReference::DefaultBranch,
                };

                PinnedSourceSpec::Git(crate::PinnedGitSpec {
                    git: url,
                    source: PinnedGitCheckout {
                        commit: GitSha::from_str(&rev).unwrap(),
                        subdirectory: subdir.map(|s| s.to_string()),
                        reference,
                    },
                })
            }
            PackageBuildSource::Url {
                url,
                sha256,
                subdir: _,
            } => PinnedSourceSpec::Url(crate::PinnedUrlSpec {
                url,
                sha256,
                md5: None,
            }),
            PackageBuildSource::Path { path } => {
                PinnedSourceSpec::Path(crate::PinnedPathSpec { path })
            }
        });
        Ok(Self {
            name: value.name,
            manifest_source: value.location.try_into()?,
            version: value.version,
            variants: value.variants,
            depends: value.depends,
            constrains: value.constrains,
            experimental_extra_depends: value.experimental_extra_depends,
            license: value.license,
            purls: value.purls,
            input_hash: value.input.map(|hash| InputHash {
                hash: hash.hash,
                globs: BTreeSet::from_iter(hash.globs),
            }),
            build_source: pinned_source_spec,
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, SourceSpec::from(v)))
                .collect(),
            python_site_packages_path: value.python_site_packages_path,
        })
    }
}

impl Matches<SourceRecord> for MatchSpec {
    fn matches(&self, pkg: &SourceRecord) -> bool {
        // Check if the name matches
        if let Some(ref name) = self.name {
            if name != &pkg.name {
                return false;
            }
        }

        if let (Some(version_spec), Some(version)) = (&self.version, &pkg.version) {
            if !version_spec.matches(version) {
                return false;
            }
        }

        if self.channel.is_some() {
            // We don't have a channel in a source record. So if a matchspec requires that
            // information it can't match.
            return false;
        }

        // For source packages, version, build, and build_number are not stored
        // in the lock file, so we only match by name
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_git::sha::GitSha;
    use std::str::FromStr;
    use url::Url;

    #[test]
    fn package_build_source_roundtrip_preserves_git_subdirectory() {
        let git_url = Url::parse("https://example.com/repo.git").unwrap();
        let pinned_source = PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: git_url.clone(),
            source: PinnedGitCheckout {
                commit: GitSha::from_str("0123456789abcdef0123456789abcdef01234567").unwrap(),
                subdirectory: Some("nested/project".to_string()),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        let record = SourceRecord {
            name: PackageName::from_str("example").unwrap(),
            version: Some(VersionWithSource::from_str("1.0.0").unwrap()),
            manifest_source: pinned_source.clone(),
            build_source: Some(pinned_source.clone()),
            input_hash: None,
            sources: Default::default(),
            variants: Default::default(),
            constrains: Default::default(),
            depends: Default::default(),
            experimental_extra_depends: Default::default(),
            license: None,
            purls: None,
            python_site_packages_path: None,
        };

        let CondaPackageData::Source(conda_source) = record.clone().into() else {
            panic!("expected source package data");
        };

        let package_build_source = conda_source
            .package_build_source
            .as_ref()
            .expect("expected package build source");

        let PackageBuildSource::Git {
            url,
            spec,
            rev,
            subdir,
        } = package_build_source
        else {
            panic!("expected git package build source");
        };

        assert_eq!(url.path(), "/repo.git");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(subdir.as_ref().map(|s| s.as_str()), Some("nested/project"));
        assert!(matches!(spec, Some(GitShallowSpec::Branch(branch)) if branch == "main"));
        assert_eq!(rev, "0123456789abcdef0123456789abcdef01234567");

        let roundtrip = SourceRecord::try_from(conda_source).expect("roundtrip should succeed");
        let Some(PinnedSourceSpec::Git(roundtrip_git)) = roundtrip.build_source else {
            panic!("expected git pinned source");
        };
        assert_eq!(
            roundtrip_git.source.subdirectory.as_deref(),
            Some("nested/project")
        );
        assert_eq!(roundtrip_git.git, git_url);
    }
}
