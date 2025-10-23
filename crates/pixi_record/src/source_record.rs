use std::{
    collections::{BTreeSet, HashMap},
    str::FromStr,
};

use pixi_git::sha::GitSha;
use pixi_spec::{GitReference, SourceSpec};
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageRecord};
use rattler_digest::{Sha256, Sha256Hash};
use rattler_lock::{CondaPackageData, CondaSourceData, GitShallowSpec, PackageBuildSource};
use serde::{Deserialize, Serialize};
use typed_path::Utf8TypedPathBuf;

use crate::{ParseLockFileError, PinnedGitCheckout, PinnedSourceSpec};

/// A record of a conda package that still requires building.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecord {
    /// Information about the conda package. This is metadata of the package
    /// after it has been build.
    pub package_record: PackageRecord,

    /// Exact definition of the source of the package.
    pub source: PinnedSourceSpec,

    pub pinned_source_spec: Option<PinnedSourceSpec>,

    /// The hash of the input that was used to build the metadata of the
    /// package. This can be used to verify that the metadata is still valid.
    ///
    /// If this is `None`, the input hash was not computed or is not relevant
    /// for this record. The record can always be considered up to date.
    pub input_hash: Option<InputHash>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceSpec>,
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
        let package_build_source = value.pinned_source_spec.map(|s| match s {
            PinnedSourceSpec::Url(pinned_url_spec) => PackageBuildSource::Url {
                url: pinned_url_spec.url,
                sha256: pinned_url_spec.sha256,
                subdir: None,
            },
            PinnedSourceSpec::Git(pinned_git_spec) => {
                let subdirectory = pinned_git_spec
                    .source
                    .subdirectory
                    .as_deref()
                    .map(Utf8TypedPathBuf::from);

                let spec = match &pinned_git_spec.source.reference {
                    GitReference::Branch(branch) => Some(GitShallowSpec::Branch(branch.clone())),
                    GitReference::Tag(tag) => Some(GitShallowSpec::Tag(tag.clone())),
                    GitReference::Rev(_) => Some(GitShallowSpec::Rev),
                    GitReference::DefaultBranch => None,
                };

                PackageBuildSource::Git {
                    url: pinned_git_spec.git,
                    spec,
                    rev: pinned_git_spec.source.commit.to_string(),
                    subdir: subdirectory,
                }
            }
            PinnedSourceSpec::Path(pinned_path) => PackageBuildSource::Path {
                path: pinned_path.path,
            },
        });
        CondaPackageData::Source(CondaSourceData {
            package_record: value.package_record,
            location: value.source.clone().into(),
            package_build_source,
            input: value.input_hash.map(|i| rattler_lock::InputHash {
                hash: i.hash,
                // TODO: fix this in rattler
                globs: Vec::from_iter(i.globs),
            }),
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
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
            package_record: value.package_record,
            source: value.location.try_into()?,
            input_hash: value.input.map(|hash| InputHash {
                hash: hash.hash,
                globs: BTreeSet::from_iter(hash.globs),
            }),
            pinned_source_spec,
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, SourceSpec::from(v)))
                .collect(),
        })
    }
}

impl Matches<SourceRecord> for NamelessMatchSpec {
    fn matches(&self, pkg: &SourceRecord) -> bool {
        if !self.matches(&pkg.package_record) {
            return false;
        }

        if self.channel.is_some() {
            // We don't have a channel in a source record. So if a matchspec requires that
            // information it can't match.
            return false;
        }

        true
    }
}

impl Matches<SourceRecord> for MatchSpec {
    fn matches(&self, pkg: &SourceRecord) -> bool {
        if !self.matches(&pkg.package_record) {
            return false;
        }

        if self.channel.is_some() {
            // We don't have a channel in a source record. So if a matchspec requires that
            // information it can't match.
            return false;
        }

        true
    }
}

impl AsRef<PackageRecord> for SourceRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.package_record
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_git::sha::GitSha;
    use serde_json::json;
    use std::str::FromStr;
    use url::Url;

    #[test]
    fn package_build_source_roundtrip_preserves_git_subdirectory() {
        let package_record: PackageRecord = serde_json::from_value(json!({
            "name": "example",
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "subdir": "noarch",
        }))
        .expect("valid package record");

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
            package_record,
            source: pinned_source.clone(),
            pinned_source_spec: Some(pinned_source.clone()),
            input_hash: None,
            sources: Default::default(),
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
        let Some(PinnedSourceSpec::Git(roundtrip_git)) = roundtrip.pinned_source_spec else {
            panic!("expected git pinned source");
        };
        assert_eq!(
            roundtrip_git.source.subdirectory.as_deref(),
            Some("nested/project")
        );
        assert_eq!(roundtrip_git.git, git_url);
    }
}
