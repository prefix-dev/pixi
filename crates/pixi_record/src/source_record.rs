use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    str::FromStr,
};

use pixi_git::{sha::GitSha, url::RepositoryUrl};
use pixi_spec::{GitReference, SourceSpec};
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageRecord};
use rattler_digest::{Sha256, Sha256Hash};
use rattler_lock::{CondaSourceData, GitShallowSpec, PackageBuildSource};
use serde::{Deserialize, Serialize};
use typed_path::{Utf8TypedPathBuf, Utf8UnixPathBuf};
use url::Url;

use crate::{ParseLockFileError, PinnedGitCheckout, PinnedSourceSpec, VariantValue};

/// A record of a conda package that still requires building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// The variants that uniquely identify the way this package was built.
    pub variants: Option<BTreeMap<String, VariantValue>>,

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
        let package_build_source = if let Some(package_build_source) = self.build_source.clone() {
            // See if we can make it relative
            let package_build_source_path = package_build_source
                .clone()
                .make_relative_to(&self.manifest_source, workspace_root)
                .map(|path| PackageBuildSource::Path {
                    path: Utf8TypedPathBuf::Unix(path),
                });

            if package_build_source_path.is_none() {
                match package_build_source {
                    PinnedSourceSpec::Url(pinned_url_spec) => Some(PackageBuildSource::Url {
                        url: pinned_url_spec.url,
                        sha256: pinned_url_spec.sha256,
                        subdir: None,
                    }),
                    PinnedSourceSpec::Git(pinned_git_spec) => Some(PackageBuildSource::Git {
                        url: pinned_git_spec.git,
                        spec: to_git_shallow(&pinned_git_spec.source.reference),
                        rev: pinned_git_spec.source.commit.to_string(),
                        subdir: pinned_git_spec
                            .source
                            .subdirectory
                            .map(Utf8TypedPathBuf::from),
                    }),
                    PinnedSourceSpec::Path(pinned_path_spec) => Some(PackageBuildSource::Path {
                        path: pinned_path_spec.path,
                    }),
                }
            } else {
                package_build_source_path
            }
        } else {
            None
        };

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
            variants: self
                .variants
                .map(|variants| variants.into_iter().map(|(k, v)| (k, v.into())).collect()),
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
                // Check if this is a relative subdirectory (same repo checkout)
                if let (Some(subdir), PinnedSourceSpec::Git(manifest_git)) =
                    (&subdir, &manifest_source)
                {
                    if same_git_checkout_url_commit(manifest_git, &url, &rev) {
                        // The subdirectory is relative to the manifest, use from_relative_to
                        let relative_path = Utf8UnixPathBuf::from(subdir.as_str());
                        return PinnedSourceSpec::from_relative_to(
                            relative_path,
                            &manifest_source,
                            workspace_root,
                        )
                        .expect("from_relative_to should succeed for same-repo git checkouts, this is a bug");
                    }
                }

                // Different repository
                let reference = git_reference_from_shallow(spec, &rev);
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
                // Convert path to Unix format for from_relative_to
                let path_unix = match path {
                    Utf8TypedPathBuf::Unix(ref p) => p,
                    // If its a windows path, it can *ONLY* be absolute per the `into_conda_source_data` method
                    // so let's return as-is
                    Utf8TypedPathBuf::Windows(path) => {
                        return PinnedSourceSpec::Path(crate::PinnedPathSpec { path: Utf8TypedPathBuf::Windows(path) })
                    }
                };

                // Try to resolve relative to manifest_source, or use absolute path if that fails
                PinnedSourceSpec::from_relative_to(path_unix.to_path_buf(), &manifest_source, workspace_root)
                    .unwrap_or(
                        // If from_relative_to returns None (absolute paths), use as-is
                        PinnedSourceSpec::Path(crate::PinnedPathSpec { path })
                    )
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
            variants: data.variants.map(|variants| {
                variants
                    .into_iter()
                    .map(|(k, v)| (k, VariantValue::from(v)))
                    .collect()
            }),
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

/// Returns true when the git URL and commit match the manifest git spec.
/// Used while parsing lock data where only the URL + rev string are available.
fn same_git_checkout_url_commit(manifest_git: &crate::PinnedGitSpec, url: &Url, rev: &str) -> bool {
    RepositoryUrl::new(&manifest_git.git) == RepositoryUrl::new(url)
        && manifest_git.source.commit.to_string() == rev
}

fn to_git_shallow(reference: &GitReference) -> Option<GitShallowSpec> {
    match reference {
        GitReference::Branch(branch) => Some(GitShallowSpec::Branch(branch.clone())),
        GitReference::Tag(tag) => Some(GitShallowSpec::Tag(tag.clone())),
        GitReference::Rev(_) => Some(GitShallowSpec::Rev),
        GitReference::DefaultBranch => None,
    }
}

fn git_reference_from_shallow(spec: Option<GitShallowSpec>, rev: &str) -> GitReference {
    match spec {
        Some(GitShallowSpec::Branch(branch)) => GitReference::Branch(branch),
        Some(GitShallowSpec::Tag(tag)) => GitReference::Tag(tag),
        Some(GitShallowSpec::Rev) => GitReference::Rev(rev.to_string()),
        None => GitReference::DefaultBranch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_git::sha::GitSha;
    use serde_json::json;
    use std::str::FromStr;
    use url::Url;

    use rattler_conda_types::Platform;
    use rattler_lock::{
        Channel, CondaPackageData, DEFAULT_ENVIRONMENT_NAME, LockFile, LockFileBuilder,
    };

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
            variants: None,
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

        // Because manifest + build live in the same git repo we serialize the build as a git
        // source with a subdir relative to the manifest checkout.
        assert_eq!(
            path.as_str(),
            "../src",
            "build_source should be relative to manifest_source directory"
        );

        // Convert back to SourceRecord (deserialization) and ensure we recover repo-root subdir
        let roundtrip = SourceRecord::from_conda_source_data(
            conda_source,
            &std::path::PathBuf::from("/workspace"),
        )
        .expect("roundtrip should succeed");

        let Some(PinnedSourceSpec::Path(roundtrip_path)) = roundtrip.build_source else {
            panic!("expected path pinned source");
        };

        // After roundtrip the git subdirectory should be expressed from repo root again.
        assert_eq!(roundtrip_path.path.as_str(), "src");
    }

    #[test]
    fn package_build_source_roundtrip_git_with_subdir() {
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
                commit,
                subdirectory: Some("recipes".to_string()),
                reference: GitReference::Branch("main".to_string()),
            },
        });

        // Build source is in src/ subdirectory (sibling of recipes)
        let build_source = PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: git_url.clone(),
            source: PinnedGitCheckout {
                commit,
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
            variants: None,
        };

        // Convert to CondaPackageData (serialization)
        let conda_source = record
            .clone()
            .into_conda_source_data(&std::path::PathBuf::from("/workspace"));

        let package_build_source = conda_source
            .package_build_source
            .as_ref()
            .expect("expected package build source");

        let PackageBuildSource::Path { path, .. } = package_build_source else {
            panic!("expected path build source with relative subdir");
        };

        // Path is relative to manifest checkout (recipes -> ../src)
        assert_eq!(path.as_str(), "../src");

        // Convert back to SourceRecord (deserialization)
        let roundtrip = SourceRecord::from_conda_source_data(
            conda_source,
            &std::path::PathBuf::from("/workspace"),
        )
        .expect("roundtrip should succeed");

        let Some(PinnedSourceSpec::Git(roundtrip_path)) = roundtrip.build_source else {
            panic!(
                "expected path pinned source after roundtrip (deserialized from relative path in lock file)"
            );
        };

        // After roundtrip, the path will contain .. components (not normalized)
        assert_eq!(
            roundtrip_path
                .source
                .subdirectory
                .expect("subdirectory should be set")
                .as_str(),
            "src"
        );
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
                commit: commit2,
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
            variants: None,
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

    #[test]
    fn roundtrip_conda_source_data() {
        let workspace_root = std::path::Path::new("/workspace");

        // Load the lock file from the snapshot content (skip insta frontmatter).
        let lock_source = lock_source_from_snapshot();
        let lock_file = LockFile::from_str(&lock_source).expect("failed to load lock file fixture");

        // Extract Conda source packages from the lock file.
        let environment = lock_file
            .default_environment()
            .expect("expected default environment");

        let conda_sources: Vec<CondaSourceData> = environment
            .conda_packages_by_platform()
            .flat_map(|(_, packages)| packages.filter_map(|pkg| pkg.as_source().cloned()))
            .collect();

        // Convert to SourceRecords and roundtrip back to CondaSourceData.
        let roundtrip_records: Vec<SourceRecord> = conda_sources
            .iter()
            .map(|conda_data| {
                SourceRecord::from_conda_source_data(conda_data.clone(), workspace_root)
                    .expect("from_conda_source_data should succeed")
            })
            .collect();

        let roundtrip_lock = build_lock_from_records(&roundtrip_records, workspace_root);
        let mut settings = insta::Settings::clone_current();
        settings.set_sort_maps(true);
        settings.bind(|| {
            insta::assert_snapshot!(roundtrip_lock);
        });
    }

    /// Extract the lock file body from the snapshot by skipping the insta frontmatter.
    fn lock_source_from_snapshot() -> String {
        let snapshot_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "src/snapshots/pixi_record__source_record__tests__roundtrip_conda_source_data.snap",
        );
        #[allow(clippy::disallowed_methods)]
        let snap = std::fs::read_to_string(snapshot_path).expect("failed to read snapshot file");
        // Skip insta frontmatter (two --- delimiters) and return the lock file contents
        snap.splitn(3, "---")
            .nth(2)
            .map(|s| s.trim_start_matches('\n'))
            .unwrap_or_default()
            .to_string()
    }

    /// Build a lock file string from a set of SourceRecords.
    fn build_lock_from_records(
        records: &[SourceRecord],
        workspace_root: &std::path::Path,
    ) -> String {
        let mut builder = LockFileBuilder::new();
        builder.set_channels(
            DEFAULT_ENVIRONMENT_NAME,
            [Channel::from("https://conda.anaconda.org/conda-forge/")],
        );

        for record in records {
            let conda_data =
                CondaPackageData::from(record.clone().into_conda_source_data(workspace_root));

            let platform = Platform::from_str(&conda_data.record().subdir)
                .expect("failed to parse platform from subdir");
            builder.add_conda_package(DEFAULT_ENVIRONMENT_NAME, platform, conda_data);
        }

        builder
            .finish()
            .render_to_string()
            .expect("failed to render lock file")
    }

    #[test]
    fn git_reference_conversion_helpers() {
        use super::{git_reference_from_shallow, to_git_shallow};
        use pixi_spec::GitReference;
        use rattler_lock::GitShallowSpec;

        assert!(matches!(
            to_git_shallow(&GitReference::Branch("main".into())),
            Some(GitShallowSpec::Branch(branch)) if branch == "main"
        ));

        assert!(matches!(
            to_git_shallow(&GitReference::Tag("v1".into())),
            Some(GitShallowSpec::Tag(tag)) if tag == "v1"
        ));

        assert!(matches!(
            to_git_shallow(&GitReference::Rev("abc".into())),
            Some(GitShallowSpec::Rev)
        ));

        assert!(to_git_shallow(&GitReference::DefaultBranch).is_none());

        assert!(matches!(
            git_reference_from_shallow(Some(GitShallowSpec::Branch("dev".into())), "ignored"),
            GitReference::Branch(branch) if branch == "dev"
        ));

        assert!(matches!(
            git_reference_from_shallow(Some(GitShallowSpec::Tag("v2".into())), "ignored"),
            GitReference::Tag(tag) if tag == "v2"
        ));

        assert!(matches!(
            git_reference_from_shallow(Some(GitShallowSpec::Rev), "deadbeef"),
            GitReference::Rev(rev) if rev == "deadbeef"
        ));

        assert!(matches!(
            git_reference_from_shallow(None, "deadbeef"),
            GitReference::DefaultBranch
        ));
    }
}
