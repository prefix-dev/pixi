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
        let package_build_source = value.pinned_source_spec.and_then(|s| match s {
            PinnedSourceSpec::Url(pinned_url_spec) => Some(PackageBuildSource::Url {
                url: pinned_url_spec.url,
                sha256: pinned_url_spec.sha256,
            }),
            PinnedSourceSpec::Git(pinned_git_spec) => Some(PackageBuildSource::Git {
                url: pinned_git_spec.git,
                spec: match pinned_git_spec.source.reference {
                    GitReference::Branch(branch) => Some(GitShallowSpec::Branch(branch)),
                    GitReference::Tag(tag) => Some(GitShallowSpec::Tag(tag)),
                    GitReference::Rev(_) => None,
                    GitReference::DefaultBranch => None, // Is this correct?
                },
                rev: pinned_git_spec.source.commit.to_string(),
            }),
            PinnedSourceSpec::Path(_) => None,
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
            PackageBuildSource::Git { url, spec, rev } => {
                PinnedSourceSpec::Git(crate::PinnedGitSpec {
                    git: url,
                    source: PinnedGitCheckout {
                        commit: GitSha::from_str(&rev).unwrap(),
                        subdirectory: None,
                        reference: match spec {
                            Some(GitShallowSpec::Branch(branch)) => GitReference::Branch(branch),
                            Some(GitShallowSpec::Tag(tag)) => GitReference::Tag(tag),
                            None => GitReference::DefaultBranch,
                        },
                    },
                })
            }
            PackageBuildSource::Url { url, sha256 } => {
                PinnedSourceSpec::Url(crate::PinnedUrlSpec {
                    url,
                    sha256,
                    md5: None,
                })
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
