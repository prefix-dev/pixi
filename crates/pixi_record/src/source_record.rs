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
            PinnedSourceSpec::Git(pinned_git_spec) => {
                let mut url = pinned_git_spec.git.clone();

                // Preserve existing query parameters and add the subdirectory if present.
                // TODO(remi): This is a temporary workaround. We inject the git
                // subdirectory into the URL query because rattler_lock::PackageBuildSource::Git
                // does not expose a dedicated subdirectory field yet. Once the lock schema
                // grows that field we should store the value there and delete this query
                // manipulation.
                let mut query_pairs: Vec<(String, String)> = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                if let Some(subdir) = pinned_git_spec.source.subdirectory.as_ref() {
                    // Drop any previously stored subdirectory before adding the current one.
                    query_pairs.retain(|(k, _)| k != "subdirectory");
                    query_pairs.push(("subdirectory".to_string(), subdir.clone()));
                }

                url.set_query(None);
                if !query_pairs.is_empty() {
                    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                    for (key, value) in query_pairs {
                        serializer.append_pair(&key, &value);
                    }
                    let query = serializer.finish();
                    url.set_query(Some(&query));
                }

                Some(PackageBuildSource::Git {
                    url,
                    spec: match pinned_git_spec.source.reference {
                        GitReference::Branch(branch) => Some(GitShallowSpec::Branch(branch)),
                        GitReference::Tag(tag) => Some(GitShallowSpec::Tag(tag)),
                        GitReference::Rev(_) => None,
                        GitReference::DefaultBranch => None, // Is this correct?
                    },
                    rev: pinned_git_spec.source.commit.to_string(),
                })
            }
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
                let mut clean_url = url.clone();
                let mut subdirectory = None;

                // TODO(remi): Keep this in sync with the serialization workaround above.
                // Drop this once the lock format can carry git subdirectories explicitly.
                let mut query_pairs: Vec<(String, String)> = clean_url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                if !query_pairs.is_empty() {
                    clean_url.set_query(None);
                    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                    for (key, value) in query_pairs.drain(..) {
                        if key == "subdirectory" {
                            subdirectory = Some(value);
                        } else {
                            serializer.append_pair(&key, &value);
                        }
                    }
                    let remainder = serializer.finish();
                    if !remainder.is_empty() {
                        clean_url.set_query(Some(&remainder));
                    }
                }

                PinnedSourceSpec::Git(crate::PinnedGitSpec {
                    git: clean_url,
                    source: PinnedGitCheckout {
                        commit: GitSha::from_str(&rev).unwrap(),
                        subdirectory,
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

        let lock_url = match conda_source.package_build_source.as_ref().unwrap() {
            PackageBuildSource::Git { url, .. } => url.clone(),
            _ => panic!("expected git package build source"),
        };
        assert_eq!(lock_url.path(), "/repo.git");
        assert_eq!(lock_url.host_str(), Some("example.com"));
        assert_eq!(lock_url.query(), Some("subdirectory=nested%2Fproject"));

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
