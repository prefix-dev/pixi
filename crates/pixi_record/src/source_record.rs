use pixi_git::sha::GitSha;
use pixi_spec::{GitReference, SourceLocationSpec};
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageName, PackageRecord};
use rattler_lock::{
    CondaSourceData, FullSourceMetadata, GitShallowSpec, PackageBuildSource,
    PartialSourceMetadata, SourceMetadata,
};
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
pub struct SourceRecord<D> {
    /// Information about the conda package.
    pub data: D,

    /// Exact definition of the source of the package.
    pub manifest_source: PinnedSourceSpec,

    /// The optional pinned source where the build should be executed
    /// This is used when the manifest is not in the same location as the
    /// source files.
    pub build_source: Option<PinnedBuildSourceSpec>,

    /// The variants that uniquely identify the way this package was built.
    pub variants: BTreeMap<String, VariantValue>,

    /// The short hash that was originally parsed from the lock file (e.g.
    /// the 9f3c2a7b part of numba-cuda[9f3c2a7b] @ .).
    ///
    /// It's useful to reuse this identifier to avoid unnecessary lock-file
    /// updates. If this field is None when serializing to the lock-file, it
    /// will be regenerated based on the contents of this struct itself.
    pub identifier_hash: Option<String>,
}

/// A source record with full metadata (package record + sources).
pub type FullSourceRecord = SourceRecord<FullSourceRecordData>;

/// A source record with only the package name (no metadata resolved yet).
pub type PartialSourceRecord = SourceRecord<PartialSourceRecordData>;

/// A source record that may be full or partial.
pub type UnresolvedSourceRecord = SourceRecord<SourceRecordData>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartialSourceRecordData {
    /// The package name of the source record.
    pub name: PackageName,

    /// Dependencies on other packages (run-time requirements).
    pub depends: Vec<String>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceLocationSpec>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FullSourceRecordData {
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceLocationSpec>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SourceRecordData {
    Partial(PartialSourceRecordData),
    Full(FullSourceRecordData),
}

impl SourceRecordData {
    pub fn package_name(&self) -> &PackageName {
        match self {
            SourceRecordData::Partial(data) => &data.name,
            SourceRecordData::Full(data) => &data.package_record.name,
        }
    }

    pub fn as_partial(&self) -> Option<&PartialSourceRecordData> {
        if let SourceRecordData::Partial(data) = self {
            Some(data)
        } else {
            None
        }
    }

    pub fn as_full(&self) -> Option<&FullSourceRecordData> {
        if let SourceRecordData::Full(data) = self {
            Some(data)
        } else {
            None
        }
    }

    pub fn is_partial(&self) -> bool {
        matches!(self, SourceRecordData::Partial(_))
    }

    pub fn is_full(&self) -> bool {
        matches!(self, SourceRecordData::Full(_))
    }
}

impl<D> SourceRecord<D> {
    /// The pinned source location from the manifest.
    pub fn manifest_source(&self) -> &PinnedSourceSpec {
        &self.manifest_source
    }

    /// The optional pinned build source.
    pub fn build_source(&self) -> Option<&PinnedBuildSourceSpec> {
        self.build_source.as_ref()
    }

    /// The variants that identify how this package was built.
    pub fn variants(&self) -> &BTreeMap<String, VariantValue> {
        &self.variants
    }
}

impl SourceRecord<FullSourceRecordData> {
    /// The name of the package.
    pub fn name(&self) -> &PackageName {
        &self.data.package_record.name
    }

    /// The full package record.
    pub fn package_record(&self) -> &PackageRecord {
        &self.data.package_record
    }

    /// Run-time dependencies.
    pub fn depends(&self) -> &[String] {
        &self.data.package_record.depends
    }

    /// Source dependency locations.
    pub fn sources(&self) -> &HashMap<String, SourceLocationSpec> {
        &self.data.sources
    }

    /// Returns true if either the manifest source or build source is mutable
    /// (i.e. path-based and may change over time).
    pub fn has_mutable_source(&self) -> bool {
        self.manifest_source.is_mutable()
            || self
                .build_source
                .as_ref()
                .is_some_and(|bs| bs.pinned().is_mutable())
    }

    /// Convert into lock-file compatible `CondaSourceData`.
    ///
    /// If either source (manifest or build) is mutable, the record is
    /// downgraded to partial metadata so the lock-file only stores minimal
    /// information for packages whose metadata may change.
    pub fn into_conda_source_data(self, workspace_root: &Path) -> CondaSourceData {
        let has_mutable = self.has_mutable_source();
        let mut unresolved = SourceRecord::<SourceRecordData>::from(self);
        if has_mutable {
            // Downgrade full data to partial — keep only name, depends, and sources.
            if let SourceRecordData::Full(full) = unresolved.data {
                unresolved.data = SourceRecordData::Partial(PartialSourceRecordData {
                    name: full.package_record.name,
                    depends: full.package_record.depends,
                    sources: full.sources,
                });
            }
        }
        unresolved.into_conda_source_data(workspace_root)
    }

    /// Returns true if this source record refers to the same output as the other source record.
    /// This is determined by comparing the package name, and either the variants (if both records have them)
    /// or the build, version and subdir (if variants are not present).
    pub fn refers_to_same_output(&self, other: &SourceRecord<FullSourceRecordData>) -> bool {
        if self.data.package_record.name != other.data.package_record.name {
            return false;
        }

        if self.variants.is_empty() || other.variants.is_empty() {
            return true;
        }

        self.variants == other.variants
    }
}

impl Matches<SourceRecord<FullSourceRecordData>> for NamelessMatchSpec {
    fn matches(&self, pkg: &SourceRecord<FullSourceRecordData>) -> bool {
        if !self.matches(&pkg.data.package_record) {
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

impl Matches<SourceRecord<FullSourceRecordData>> for MatchSpec {
    fn matches(&self, pkg: &SourceRecord<FullSourceRecordData>) -> bool {
        if !self.matches(&pkg.data.package_record) {
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

impl AsRef<PackageRecord> for SourceRecord<FullSourceRecordData> {
    fn as_ref(&self) -> &PackageRecord {
        &self.data.package_record
    }
}

impl SourceRecord<PartialSourceRecordData> {
    /// The name of the package.
    pub fn name(&self) -> &PackageName {
        &self.data.name
    }

    /// Run-time dependencies.
    pub fn depends(&self) -> &[String] {
        &self.data.depends
    }

    /// Source dependency locations.
    pub fn sources(&self) -> &HashMap<String, SourceLocationSpec> {
        &self.data.sources
    }
}

impl SourceRecord<SourceRecordData> {
    /// The name of the package.
    pub fn name(&self) -> &PackageName {
        self.data.package_name()
    }

    /// Run-time dependencies.
    pub fn depends(&self) -> &[String] {
        match &self.data {
            SourceRecordData::Full(full) => &full.package_record.depends,
            SourceRecordData::Partial(partial) => &partial.depends,
        }
    }

    /// Source dependency locations.
    pub fn sources(&self) -> &HashMap<String, SourceLocationSpec> {
        match &self.data {
            SourceRecordData::Full(full) => &full.sources,
            SourceRecordData::Partial(partial) => &partial.sources,
        }
    }

    /// Convert into lock-file compatible `CondaSourceData<SourceMetadata>`.
    pub fn into_conda_source_data(self, _workspace_root: &Path) -> CondaSourceData {
        let package_build_source = build_source_to_package_build_source(self.build_source);

        let metadata = match self.data {
            SourceRecordData::Full(full) => SourceMetadata::Full(Box::new(FullSourceMetadata {
                package_record: full.package_record,
                sources: full.sources.into_iter().map(|(k, v)| (k, v.into())).collect(),
            })),
            SourceRecordData::Partial(partial) => {
                SourceMetadata::Partial(PartialSourceMetadata {
                    name: partial.name,
                    depends: partial.depends,
                    sources: partial.sources.into_iter().map(|(k, v)| (k, v.into())).collect(),
                })
            }
        };

        CondaSourceData {
            location: self.manifest_source.clone().into(),
            package_build_source,
            variants: self
                .variants
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            identifier_hash: self.identifier_hash,
            metadata,
        }
    }

    /// Create from lock-file `CondaSourceData<SourceMetadata>`.
    pub fn from_conda_source_data(
        data: CondaSourceData,
        _workspace_root: &std::path::Path,
    ) -> Result<Self, ParseLockFileError> {
        let manifest_source: PinnedSourceSpec = data.location.try_into()?;
        let build_source =
            package_build_source_to_build_source(data.package_build_source, &manifest_source)?;

        let record_data = match data.metadata {
            SourceMetadata::Full(full) => SourceRecordData::Full(FullSourceRecordData {
                package_record: full.package_record,
                sources: full
                    .sources
                    .into_iter()
                    .map(|(k, v)| (k, SourceLocationSpec::from(v)))
                    .collect(),
            }),
            SourceMetadata::Partial(partial) => {
                SourceRecordData::Partial(PartialSourceRecordData {
                    name: partial.name,
                    depends: partial.depends,
                    sources: partial.sources.into_iter().map(|(k, v)| (k, SourceLocationSpec::from(v))).collect(),
                })
            }
        };

        Ok(Self {
            data: record_data,
            manifest_source,
            build_source,
            variants: data
                .variants
                .into_iter()
                .map(|(k, v)| (k, VariantValue::from(v)))
                .collect(),
            identifier_hash: data.identifier_hash,
        })
    }
}

/// Upcast from full to unresolved.
impl From<SourceRecord<FullSourceRecordData>> for SourceRecord<SourceRecordData> {
    fn from(record: SourceRecord<FullSourceRecordData>) -> Self {
        Self {
            data: SourceRecordData::Full(record.data),
            manifest_source: record.manifest_source,
            build_source: record.build_source,
            variants: record.variants,
            identifier_hash: record.identifier_hash,
        }
    }
}

/// Convert build source to rattler's PackageBuildSource.
fn build_source_to_package_build_source(
    build_source: Option<PinnedBuildSourceSpec>,
) -> Option<PackageBuildSource> {
    match build_source {
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
    }
}

/// Convert rattler's PackageBuildSource back to PinnedBuildSourceSpec.
fn package_build_source_to_build_source(
    pbs: Option<PackageBuildSource>,
    manifest_source: &PinnedSourceSpec,
) -> Result<Option<PinnedBuildSourceSpec>, ParseLockFileError> {
    match pbs {
        None => Ok(None),
        Some(PackageBuildSource::Path { path }) if path.is_relative() => {
            let pinned = manifest_source.join(path.to_path());
            Ok(Some(PinnedBuildSourceSpec::Relative(
                path.to_string(),
                pinned,
            )))
        }
        Some(PackageBuildSource::Path { path }) => Ok(Some(PinnedBuildSourceSpec::Absolute(
            PinnedSourceSpec::Path(PinnedPathSpec { path }),
        ))),
        Some(PackageBuildSource::Git {
            url,
            spec,
            rev,
            subdir,
        }) => {
            let reference = git_reference_from_shallow(spec, &rev);
            Ok(Some(PinnedBuildSourceSpec::Absolute(
                PinnedSourceSpec::Git(PinnedGitSpec {
                    git: url,
                    source: PinnedGitCheckout {
                        commit: GitSha::from_str(&rev).unwrap(),
                        subdirectory: subdir
                            .and_then(|s| pixi_spec::Subdirectory::try_from(s.to_string()).ok())
                            .unwrap_or_default(),
                        reference,
                    },
                }),
            )))
        }
        Some(PackageBuildSource::Url {
            url,
            sha256,
            subdir,
        }) => Ok(Some(PinnedBuildSourceSpec::Absolute(
            PinnedSourceSpec::Url(PinnedUrlSpec {
                url,
                sha256,
                md5: None,
                subdirectory: subdir
                    .and_then(|s| pixi_spec::Subdirectory::try_from(s.to_string()).ok())
                    .unwrap_or_default(),
            }),
        ))),
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

    type SourceRecord = super::SourceRecord<FullSourceRecordData>;

    #[test]
    fn roundtrip_conda_source_data() {
        let workspace_root = Path::new("/workspace");

        // Load the lock file from a static fixture with full metadata for all records.
        let lock_source = lock_source_from_fixture();
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

        // Convert to full SourceRecords (input fixture always has full metadata).
        let roundtrip_records: Vec<SourceRecord> = conda_sources
            .iter()
            .map(|conda_data| {
                let unresolved = super::SourceRecord::<SourceRecordData>::from_conda_source_data(
                    conda_data.clone(),
                    workspace_root,
                )
                .expect("from_conda_source_data should succeed");
                match unresolved.data {
                    SourceRecordData::Full(full) => super::SourceRecord {
                        data: full,
                        manifest_source: unresolved.manifest_source,
                        build_source: unresolved.build_source,
                        variants: unresolved.variants,
                        identifier_hash: unresolved.identifier_hash,
                    },
                    SourceRecordData::Partial(_) => {
                        panic!("fixture should only contain full source records")
                    }
                }
            })
            .collect();

        // Write back — mutable (path) records should become partial,
        // immutable (git) records stay full.
        let roundtrip_lock = build_lock_from_records(&roundtrip_records, workspace_root);
        let mut settings = insta::Settings::clone_current();
        settings.set_sort_maps(true);
        settings.bind(|| {
            insta::assert_snapshot!(roundtrip_lock);
        });
    }

    /// Load the lock file body from a static fixture file with full metadata.
    fn lock_source_from_fixture() -> String {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/test_fixtures/full_source_records.lock");
        #[allow(clippy::disallowed_methods)]
        std::fs::read_to_string(fixture_path).expect("failed to read fixture file")
    }

    /// Build a lock file string from a set of SourceRecords.
    fn build_lock_from_records(records: &[SourceRecord], workspace_root: &Path) -> String {
        // Collect all unique platforms from the records (using the package_record's subdir).
        let platforms: std::collections::HashSet<Platform> = records
            .iter()
            .map(|r| {
                Platform::from_str(&r.package_record().subdir)
                    .expect("failed to parse platform from subdir")
            })
            .collect();

        let mut builder = LockFileBuilder::new()
            .with_platforms(
                platforms
                    .iter()
                    .map(|p| rattler_lock::PlatformData {
                        name: rattler_lock::PlatformName::from(p),
                        subdir: *p,
                        virtual_packages: Vec::new(),
                    })
                    .collect(),
            )
            .expect("platforms should be unique");
        builder.set_channels(
            DEFAULT_ENVIRONMENT_NAME,
            [Channel::from("https://conda.anaconda.org/conda-forge/")],
        );

        for record in records {
            let platform = Platform::from_str(&record.package_record().subdir)
                .expect("failed to parse platform from subdir");
            let conda_data =
                CondaPackageData::from(record.clone().into_conda_source_data(workspace_root));
            builder
                .add_conda_package(DEFAULT_ENVIRONMENT_NAME, &platform.to_string(), conda_data)
                .expect("platform was registered");
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

    #[test]
    fn partial_source_record_roundtrip() {
        use crate::{PinnedPathSpec, PinnedSourceSpec};

        let workspace_root = Path::new("/workspace");

        // Create a partial source record.
        let partial = super::SourceRecord::<SourceRecordData> {
            data: SourceRecordData::Partial(PartialSourceRecordData {
                name: PackageName::from_str("my-package").unwrap(),
                depends: vec!["numpy >=1.0".to_string()],
                sources: HashMap::new(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: typed_path::Utf8TypedPathBuf::from("./my-package"),
            }),
            build_source: None,
            variants: BTreeMap::from([("python".into(), crate::VariantValue::from("3.12".to_string()))]),
            identifier_hash: Some("abcd1234".to_string()),
        };

        assert_eq!(partial.name().as_source(), "my-package");

        // Roundtrip through CondaSourceData.
        let conda_data = partial.into_conda_source_data(workspace_root);
        let roundtripped =
            super::SourceRecord::<SourceRecordData>::from_conda_source_data(
                conda_data,
                workspace_root,
            )
            .expect("from_conda_source_data should succeed");

        assert_eq!(roundtripped.name().as_source(), "my-package");
        assert!(roundtripped.data.is_partial());
        assert_eq!(
            roundtripped.variants.get("python").map(|v| v.to_string()),
            Some("3.12".to_string())
        );
        assert_eq!(roundtripped.identifier_hash.as_deref(), Some("abcd1234"));
    }

    #[test]
    fn try_into_resolved_with_full_record() {
        use crate::{PixiRecord, UnresolvedPixiRecord};

        let workspace_root = Path::new("/workspace");

        let lock_source = lock_source_from_fixture();
        let lock_file =
            LockFile::from_str_with_base_directory(&lock_source, Some(Path::new("/workspace")))
                .expect("failed to load lock file fixture");

        let environment = lock_file
            .default_environment()
            .expect("expected default environment");

        let conda_source = environment
            .conda_packages_by_platform()
            .flat_map(|(_, packages)| packages.filter_map(|pkg| pkg.as_source().cloned()))
            .next()
            .expect("expected at least one source package");

        let unresolved =
            UnresolvedPixiRecord::from_conda_package_data(
                CondaPackageData::Source(conda_source),
                workspace_root,
            )
            .expect("from_conda_package_data should succeed");

        let resolved = unresolved.try_into_resolved();
        assert!(resolved.is_ok());
        assert!(matches!(resolved.unwrap(), PixiRecord::Source(_)));
    }

    #[test]
    fn try_into_resolved_with_partial_record() {
        use crate::{PinnedPathSpec, PinnedSourceSpec, UnresolvedPixiRecord};

        let partial = UnresolvedPixiRecord::Source(super::SourceRecord::<SourceRecordData> {
            data: SourceRecordData::Partial(PartialSourceRecordData {
                name: PackageName::from_str("partial-pkg").unwrap(),
                depends: vec![],
                sources: HashMap::new(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: typed_path::Utf8TypedPathBuf::from("./partial-pkg"),
            }),
            build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
        });

        let result = partial.try_into_resolved();
        assert!(result.is_err());
        let still_partial = result.unwrap_err();
        assert_eq!(still_partial.name().as_source(), "partial-pkg");
    }

    #[test]
    fn full_upcast_roundtrip() {
        let workspace_root = Path::new("/workspace");

        // Load a full record from snapshot.
        let lock_source = lock_source_from_fixture();
        let lock_file =
            LockFile::from_str_with_base_directory(&lock_source, Some(Path::new("/workspace")))
                .expect("failed to load lock file fixture");

        let environment = lock_file
            .default_environment()
            .expect("expected default environment");

        let conda_source = environment
            .conda_packages_by_platform()
            .flat_map(|(_, packages)| packages.filter_map(|pkg| pkg.as_source().cloned()))
            .next()
            .expect("expected at least one source package");

        // Parse as unresolved record (first record in fixture is git = immutable = full).
        let unresolved =
            super::SourceRecord::<SourceRecordData>::from_conda_source_data(
                conda_source,
                workspace_root,
            )
            .expect("from_conda_source_data should succeed");
        assert!(unresolved.data.is_full());

        // Roundtrip through CondaSourceData.
        let conda_data = unresolved.into_conda_source_data(workspace_root);
        let roundtripped =
            super::SourceRecord::<SourceRecordData>::from_conda_source_data(
                conda_data,
                workspace_root,
            )
            .expect("roundtrip should succeed");

        assert!(roundtripped.data.is_full());
    }
}
