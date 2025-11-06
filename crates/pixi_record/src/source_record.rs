use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    str::FromStr,
};

use pixi_git::sha::GitSha;
use pixi_spec::{GitReference, SourceSpec};
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageRecord};
use rattler_digest::{Sha256, Sha256Hash};
use rattler_lock::{CondaSourceData, GitShallowSpec, PackageBuildSource};
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
    pub manifest_source: PinnedSourceSpec,

    /// The optional pinned source where the build should be executed
    /// This is used when the manifest is not in the same location as the
    /// source files.
    pub build_source: Option<PinnedSourceSpec>,

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

impl SourceRecord {
    /// Convert [`SourceRecord`] into lock-file compatible `CondaSourceData`
    /// The `build_source` in the SourceRecord is always relative to the workspace.
    /// However, when saving in the lock-file make these relative to the package manifest.
    /// This should be used when writing to the lock file.
    pub fn into_conda_source_data(self, workspace_root: &Path) -> CondaSourceData {
        let package_build_source = self.build_source.map(|build_source| {
            let pinned_source_spec = build_source
                .make_relative_to(&self.manifest_source, workspace_root)
                .unwrap_or(build_source);

            match pinned_source_spec {
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
                        GitReference::Branch(branch) => {
                            Some(GitShallowSpec::Branch(branch.clone()))
                        }
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
            }
        });

        CondaSourceData {
            package_record: self.package_record,
            location: self.manifest_source.clone().into(),
            package_build_source,
            input: self.input_hash.map(|i| rattler_lock::InputHash {
                hash: i.hash,
                globs: Vec::from_iter(i.globs),
            }),
            sources: self
                .sources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }

    /// Create SourceRecord from CondaSourceData with paths resolved relative to workspace_root.
    /// This should be used when reading from the lock file.
    ///
    /// The inverse of `into_conda_source_data`:
    /// - manifest_source: relative to workspace_root (or absolute) → resolve to absolute
    /// - build_source: relative to manifest_source (or absolute) → resolve to absolute
    pub fn from_conda_source_data(
        data: CondaSourceData,
        workspace_root: &std::path::Path,
    ) -> Result<Self, ParseLockFileError> {
        let manifest_source: PinnedSourceSpec = data.location.try_into()?;

        let build_source = data.package_build_source.map(|source| match source {
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

                // Out-of-tree git repository
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
                if path.is_absolute() {
                    crate::PinnedSourceSpec::Path(crate::PinnedPathSpec { path })
                } else {
                    match &manifest_source {
                        PinnedSourceSpec::Url(_) => unimplemented!(),
                        PinnedSourceSpec::Git(pinned_git_spec) => {
                            let base_dir = pinned_git_spec.source.subdirectory.as_ref();
                            // Make `path` relative to repository root, if `base` is not None it will
                            // be relative with `base` at this point, so lets join
                            let subdir =
                                base_dir.map(|base| Path::new(base).join(Path::new(path.as_str())));

                            let mut git_source = pinned_git_spec.source.clone();
                            git_source.subdirectory =
                                subdir.map(|p| p.to_string_lossy().to_string());

                            // Reconstruct the git object
                            PinnedSourceSpec::Git(crate::PinnedGitSpec {
                                git: pinned_git_spec.git.clone(),
                                source: git_source,
                            })
                        }
                        PinnedSourceSpec::Path(manifest_path) => {
                            // path is relative to manifest_source (or absolute) in the lock file
                            // We need to make it relative to workspace_root (or keep it absolute)
                            let build_source_spec = crate::PinnedPathSpec { path };

                            // If path is relative, it's relative to manifest_source
                            // First resolve manifest_source against workspace_root
                            // Then resolve path against the resolved manifest_source
                            // Finally make the result relative to workspace_root
                            let manifest_absolute = manifest_path.resolve(workspace_root);
                            let build_absolute = build_source_spec.resolve(&manifest_absolute);

                            // Make the normalized path relative to workspace_root
                            let relative_to_workspace =
                                pathdiff::diff_paths(&build_absolute, workspace_root)
                                    .unwrap_or_else(|| build_absolute);

                            PinnedSourceSpec::Path(crate::PinnedPathSpec {
                                path: Utf8TypedPathBuf::from(
                                    relative_to_workspace.to_string_lossy().as_ref(),
                                ),
                            })
                        }
                    }
                }
            }
        });

        Ok(Self {
            package_record: data.package_record,
            manifest_source,
            input_hash: data.input.map(|hash| InputHash {
                hash: hash.hash,
                globs: BTreeSet::from_iter(hash.globs),
            }),
            build_source,
            sources: data
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
            manifest_source: pinned_source.clone(),
            build_source: Some(pinned_source.clone()),
            input_hash: None,
            sources: Default::default(),
        };

        let conda_source = record
            .clone()
            .into_conda_source_data(&std::path::PathBuf::from("/workspace"));

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

        let roundtrip = SourceRecord::from_conda_source_data(
            conda_source,
            &std::path::PathBuf::from("/workspace"),
        )
        .expect("roundtrip should succeed");
        let Some(PinnedSourceSpec::Git(roundtrip_git)) = roundtrip.build_source else {
            panic!("expected git pinned source");
        };
        assert_eq!(
            roundtrip_git.source.subdirectory.as_deref(),
            Some("nested/project")
        );
        assert_eq!(roundtrip_git.git, git_url);
    }

    #[test]
    fn package_build_source_path_is_made_relative() {
        use typed_path::Utf8TypedPathBuf;

        let package_record: PackageRecord = serde_json::from_value(json!({
            "name": "example",
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "subdir": "noarch",
        }))
        .expect("valid package record");

        // Manifest is in /workspace/recipes directory
        let manifest_source = PinnedSourceSpec::Path(crate::PinnedPathSpec {
            path: Utf8TypedPathBuf::from("/workspace/recipes"),
        });

        // Build source is in /workspace/src (sibling of recipes)
        let build_source = PinnedSourceSpec::Path(crate::PinnedPathSpec {
            path: Utf8TypedPathBuf::from("/workspace/src"),
        });

        let record = SourceRecord {
            package_record,
            manifest_source: manifest_source.clone(),
            build_source: Some(build_source),
            input_hash: None,
            sources: Default::default(),
        };

        // Convert to CondaPackageData (serialization)
        let conda_source = record
            .clone()
            .into_conda_source_data(&std::path::PathBuf::from("/workspace"));

        let package_build_source = conda_source
            .package_build_source
            .as_ref()
            .expect("expected package build source");

        let PackageBuildSource::Path { path } = package_build_source else {
            panic!("expected path package build source");
        };

        // The path should now be relative to the manifest directory
        // Manifest is in /workspace/recipes/, so ../src should point to /workspace/src
        assert_eq!(
            path.as_str(),
            "../src",
            "build_source should be relative to manifest_source directory"
        );

        // Convert back to SourceRecord (deserialization)
        let roundtrip = SourceRecord::from_conda_source_data(
            conda_source,
            &std::path::PathBuf::from("/workspace"),
        )
        .expect("roundtrip should succeed");

        let Some(PinnedSourceSpec::Path(roundtrip_path)) = roundtrip.build_source else {
            panic!("expected path pinned source");
        };

        // After roundtrip, the path should remain as it was in the lock file (relative)
        assert_eq!(
            roundtrip_path.path.as_str(),
            "../src",
            "build_source should remain as relative path from lock file"
        );
    }

    #[test]
    fn package_build_source_path_stays_absolute_when_not_related() {
        use typed_path::Utf8TypedPathBuf;

        let package_record: PackageRecord = serde_json::from_value(json!({
            "name": "example",
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "subdir": "noarch",
        }))
        .expect("valid package record");

        // Manifest is in /workspace/recipes directory
        let manifest_source = PinnedSourceSpec::Path(crate::PinnedPathSpec {
            path: Utf8TypedPathBuf::from("/workspace/recipes"),
        });

        // Build source is in a completely different location
        let build_source = PinnedSourceSpec::Path(crate::PinnedPathSpec {
            path: Utf8TypedPathBuf::from("/completely/different/path"),
        });

        let record = SourceRecord {
            package_record,
            manifest_source: manifest_source.clone(),
            build_source: Some(build_source),
            input_hash: None,
            sources: Default::default(),
        };

        // Convert to CondaPackageData (serialization)
        let conda_source = record
            .clone()
            .into_conda_source_data(&std::path::PathBuf::from("/workspace"));

        let package_build_source = conda_source
            .package_build_source
            .as_ref()
            .expect("expected package build source");

        let PackageBuildSource::Path { path } = package_build_source else {
            panic!("expected path package build source");
        };

        // The path should be made relative
        assert_eq!(path.as_str(), "../../completely/different/path");
    }

    #[test]
    fn package_build_source_git_same_repo_is_made_relative() {
        let package_record: PackageRecord = serde_json::from_value(json!({
            "name": "example",
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "subdir": "noarch",
        }))
        .expect("valid package record");

        let git_url = Url::parse("https://github.com/user/repo.git").unwrap();
        let commit = GitSha::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();

        // Manifest is in recipes/ subdirectory
        let manifest_source = PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: git_url.clone(),
            source: PinnedGitCheckout {
                commit: commit.clone(),
                subdirectory: Some("recipes".to_string()),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        // Build source is in src/ subdirectory (sibling of recipes)
        let build_source = PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: git_url.clone(),
            source: PinnedGitCheckout {
                commit: commit.clone(),
                subdirectory: Some("src".to_string()),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        let record = SourceRecord {
            package_record,
            manifest_source: manifest_source.clone(),
            build_source: Some(build_source),
            input_hash: None,
            sources: Default::default(),
        };

        // Convert to CondaPackageData (serialization)
        let conda_source = record
            .clone()
            .into_conda_source_data(&std::path::PathBuf::from("/workspace"));

        let package_build_source = conda_source
            .package_build_source
            .as_ref()
            .expect("expected package build source");

        let PackageBuildSource::Path { path } = package_build_source else {
            panic!("expected path package build source (relativized from git)");
        };

        // The path should be relative: from recipes/ to src/ is ../src
        assert_eq!(
            path.as_str(),
            "../src",
            "build_source should be converted to relative path"
        );

        // Convert back to SourceRecord (deserialization)
        let roundtrip = SourceRecord::from_conda_source_data(
            conda_source,
            &std::path::PathBuf::from("/workspace"),
        )
        .expect("roundtrip should succeed");

        let Some(PinnedSourceSpec::Path(roundtrip_path)) = roundtrip.build_source else {
            panic!(
                "expected path pinned source after roundtrip (deserialized from relative path in lock file)"
            );
        };

        // After roundtrip, it should remain as the relative path from the lock file
        assert_eq!(roundtrip_path.path.as_str(), "../src");
    }

    #[test]
    fn package_build_source_git_different_repos_stays_git() {
        let package_record: PackageRecord = serde_json::from_value(json!({
            "name": "example",
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "subdir": "noarch",
        }))
        .expect("valid package record");

        let manifest_git_url = Url::parse("https://github.com/user/repo1.git").unwrap();
        let build_git_url = Url::parse("https://github.com/user/repo2.git").unwrap();
        let commit1 = GitSha::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        let commit2 = GitSha::from_str("abcdef0123456789abcdef0123456789abcdef01").unwrap();

        // Manifest is in one repository
        let manifest_source = PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: manifest_git_url.clone(),
            source: PinnedGitCheckout {
                commit: commit1,
                subdirectory: Some("recipes".to_string()),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        // Build source is in a different repository
        let build_source = PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: build_git_url.clone(),
            source: PinnedGitCheckout {
                commit: commit2.clone(),
                subdirectory: Some("src".to_string()),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        let record = SourceRecord {
            package_record,
            manifest_source: manifest_source.clone(),
            build_source: Some(build_source),
            input_hash: None,
            sources: Default::default(),
        };

        // Convert to CondaPackageData (serialization)
        let conda_source = record
            .clone()
            .into_conda_source_data(&std::path::PathBuf::from("/workspace"));

        let package_build_source = conda_source
            .package_build_source
            .as_ref()
            .expect("expected package build source");

        let PackageBuildSource::Git { url, subdir, .. } = package_build_source else {
            panic!("expected git package build source (different repos should stay git)");
        };

        // Different repositories - should stay as Git source
        assert_eq!(url, &build_git_url);
        assert_eq!(subdir.as_ref().map(|s| s.as_str()), Some("src"));
    }
}
