use pixi_git::sha::GitSha;
use pixi_spec::{GitReference, SourceLocationSpec};
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageRecord};
use rattler_lock::{CondaSourceData, GitShallowSpec, PackageBuildSource};
use std::fmt::{Display, Formatter};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    str::FromStr,
};
use typed_path::Utf8TypedPathBuf;

use crate::{
    ParseLockFileError, PinnedGitCheckout, PinnedGitSpec, PinnedPathSpec, PinnedSourceSpec,
    PinnedUrlSpec, VariantValue,
};

/// Represents a pinned build source with information about how it was originally specified in the
/// manifest.
///
/// When a build source is specified as a relative path (e.g., `../src`), we preserve the original
/// relative path for lock file serialization. Without this, we couldn't distinguish between a path
/// that was originally relative vs. absolute when the resolved path lies outside the workspace.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PinnedBuildSourceSpec {
    Absolute(PinnedSourceSpec),
    Relative(String, PinnedSourceSpec),
}

impl Display for PinnedBuildSourceSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absolute(spec) => write!(f, "{spec}"),
            Self::Relative(relative, spec) => write!(f, "{spec} ({relative})"),
        }
    }
}

impl PinnedBuildSourceSpec {
    pub fn pinned(&self) -> &PinnedSourceSpec {
        match self {
            PinnedBuildSourceSpec::Absolute(pinned) => pinned,
            PinnedBuildSourceSpec::Relative(_, pinned) => pinned,
        }
    }

    pub fn into_pinned(self) -> PinnedSourceSpec {
        match self {
            PinnedBuildSourceSpec::Absolute(pinned) => pinned,
            PinnedBuildSourceSpec::Relative(_, pinned) => pinned,
        }
    }

    pub fn pinned_mut(&mut self) -> &mut PinnedSourceSpec {
        match self {
            PinnedBuildSourceSpec::Absolute(pinned) => pinned,
            PinnedBuildSourceSpec::Relative(_, pinned) => pinned,
        }
    }
}

impl From<PinnedBuildSourceSpec> for PinnedSourceSpec {
    fn from(pinned: PinnedBuildSourceSpec) -> Self {
        pinned.into_pinned()
    }
}

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
    pub build_source: Option<PinnedBuildSourceSpec>,

    /// The variants that uniquely identify the way this package was built.
    pub variants: BTreeMap<String, VariantValue>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceLocationSpec>,
}

impl SourceRecord {
    /// Convert [`SourceRecord`] into lock-file compatible `CondaSourceData`
    /// The `build_source` in the SourceRecord is always relative to the workspace.
    /// However, when saving in the lock-file make these relative to the package manifest.
    /// This should be used when writing to the lock file.
    pub fn into_conda_source_data(self, _workspace_root: &Path) -> CondaSourceData {
        let package_build_source = match self.build_source {
            Some(PinnedBuildSourceSpec::Relative(path, _)) => Some(PackageBuildSource::Path {
                path: Utf8TypedPathBuf::from(path),
            }),
            Some(PinnedBuildSourceSpec::Absolute(PinnedSourceSpec::Url(pinned_url_spec))) => {
                Some(PackageBuildSource::Url {
                    url: pinned_url_spec.url,
                    sha256: pinned_url_spec.sha256,
                    subdir: pinned_url_spec
                        .subdirectory
                        .to_option_string()
                        .map(Utf8TypedPathBuf::from),
                })
            }
            Some(PinnedBuildSourceSpec::Absolute(PinnedSourceSpec::Git(pinned_git_spec))) => {
                Some(PackageBuildSource::Git {
                    url: pinned_git_spec.git,
                    spec: to_git_shallow(&pinned_git_spec.source.reference),
                    rev: pinned_git_spec.source.commit.to_string(),
                    subdir: pinned_git_spec
                        .source
                        .subdirectory
                        .to_option_string()
                        .map(Utf8TypedPathBuf::from),
                })
            }
            Some(PinnedBuildSourceSpec::Absolute(PinnedSourceSpec::Path(pinned_path_spec))) => {
                Some(PackageBuildSource::Path {
                    path: pinned_path_spec.path,
                })
            }
            None => None,
        };

        CondaSourceData {
            package_record: self.package_record,
            location: self.manifest_source.clone().into(),
            package_build_source,
            sources: self
                .sources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            variants: self
                .variants
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
        _workspace_root: &std::path::Path,
    ) -> Result<Self, ParseLockFileError> {
        let manifest_source: PinnedSourceSpec = data.location.try_into()?;
        let build_source = match data.package_build_source {
            None => None,
            Some(PackageBuildSource::Path { path }) if path.is_relative() => {
                let pinned = manifest_source.join(path.to_path());
                Some(PinnedBuildSourceSpec::Relative(path.to_string(), pinned))
            }
            Some(PackageBuildSource::Path { path }) => Some(PinnedBuildSourceSpec::Absolute(
                PinnedSourceSpec::Path(PinnedPathSpec { path }),
            )),
            Some(PackageBuildSource::Git {
                url,
                spec,
                rev,
                subdir,
            }) => {
                let reference = git_reference_from_shallow(spec, &rev);
                Some(PinnedBuildSourceSpec::Absolute(PinnedSourceSpec::Git(
                    PinnedGitSpec {
                        git: url,
                        source: PinnedGitCheckout {
                            commit: GitSha::from_str(&rev).unwrap(),
                            subdirectory: subdir
                                .and_then(|s| pixi_spec::Subdirectory::try_from(s.to_string()).ok())
                                .unwrap_or_default(),
                            reference,
                        },
                    },
                )))
            }
            Some(PackageBuildSource::Url {
                url,
                sha256,
                subdir,
            }) => Some(PinnedBuildSourceSpec::Absolute(PinnedSourceSpec::Url(
                PinnedUrlSpec {
                    url,
                    sha256,
                    md5: None,
                    subdirectory: subdir
                        .and_then(|s| pixi_spec::Subdirectory::try_from(s.to_string()).ok())
                        .unwrap_or_default(),
                },
            ))),
        };

        Ok(Self {
            package_record: data.package_record,
            manifest_source,
            build_source,
            sources: data
                .sources
                .into_iter()
                .map(|(k, v)| (k, SourceLocationSpec::from(v)))
                .collect(),
            variants: data
                .variants
                .into_iter()
                .map(|(k, v)| (k, VariantValue::from(v)))
                .collect(),
        })
    }

    /// Returns true if this source record refers to the same output as the other source record.
    /// This is determined by comparing the package name, and either the variants (if both records have them)
    /// or the build, version and subdir (if variants are not present).
    pub fn refers_to_same_output(&self, other: &SourceRecord) -> bool {
        if self.package_record.name != other.package_record.name {
            return false;
        }

        if self.variants.is_empty() || other.variants.is_empty() {
            return true;
        }

        self.variants == other.variants
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
    use std::{path::Path, str::FromStr};

    use rattler_conda_types::Platform;
    use rattler_lock::{
        Channel, CondaPackageData, DEFAULT_ENVIRONMENT_NAME, LockFile, LockFileBuilder,
    };

    #[test]
    fn roundtrip_conda_source_data() {
        let workspace_root = Path::new("/workspace");

        // Load the lock file from the snapshot content (skip insta frontmatter).
        let lock_source = lock_source_from_snapshot();
        let lock_file =
            LockFile::from_str_with_base_directory(&lock_source, Some(Path::new("/workspace")))
                .expect("failed to load lock file fixture");

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
        let snapshot_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(
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
    fn build_lock_from_records(records: &[SourceRecord], workspace_root: &Path) -> String {
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
