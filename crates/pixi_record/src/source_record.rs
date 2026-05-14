//! Source records for conda packages that require building from source.
//!
//! # Full vs Partial vs Unresolved
//!
//! Source records exist in three states:
//!
//! - **Full** ([`FullSourceRecord`]): all metadata is available (package record,
//!   dependencies, sources). Safe to use for building and installing.
//! - **Partial** ([`PartialSourceRecord`]): only minimal metadata (name, depends,
//!   sources). Produced when a *mutable* (path-based) source is written to the
//!   lock file, because the full metadata would be stale by the next read.
//! - **Unresolved** ([`UnresolvedSourceRecord`]): may be either full or partial.
//!   This is what the lock file produces on read.
//!
//! # State transitions
//!
//! ```text
//! Lock-file write: FullSourceRecord ──► Partial (if mutable source)
//!                                   ──► Full   (if immutable source, e.g. git)
//!
//! Lock-file read:  ──► UnresolvedSourceRecord (Full or Partial)
//!
//! Startup resolve: UnresolvedSourceRecord ──► FullSourceRecord
//!                  (re-evaluates source metadata for partial records)
//! ```
//!
//! Use [`SourceRecord::map_data`] and [`SourceRecord::try_map_data`] for clean
//! state transitions without field-by-field reconstruction.

use pixi_git::sha::GitSha;
use pixi_spec::{GitReference, SourceLocationSpec};
use rattler_conda_types::{
    Flag, MatchSpec, Matches, NamelessMatchSpec, PackageName, PackageRecord, PackageUrl,
    package::RunExportsJson,
};
use rattler_lock::{
    CondaSourceData, GitShallowSpec, PackageBuildSource, PartialSourceMetadata, SourceData,
    SourceMetadata,
};
use std::fmt::{Display, Formatter};
use std::{
    collections::{BTreeMap, BTreeSet},
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
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// the `9f3c2a7b` part of `numba-cuda[9f3c2a7b] @ .`).
    ///
    /// It's useful to reuse this identifier to avoid unnecessary lock-file
    /// updates. If this field is None when serializing to the lock-file, it
    /// will be regenerated based on the contents of this struct itself.
    pub identifier_hash: Option<String>,

    /// Packages in the build environment used to build this source package
    /// (compilers, build tools, etc.), as recorded in the lock file.
    ///
    /// Skipped by serde: `UnresolvedPixiRecord` does not implement
    /// `Serialize`/`Deserialize`; the canonical form lives in the lock file
    /// via `rattler_lock::SourceData`'s package index sets, and callers
    /// resolve them when building the in-memory record.
    #[serde(skip)]
    pub build_packages: Vec<crate::UnresolvedPixiRecord>,

    /// Packages in the host environment used to build this source package
    /// (libraries to link against, etc.), as recorded in the lock file.
    ///
    /// Skipped by serde — see `build_packages`.
    #[serde(skip)]
    pub host_packages: Vec<crate::UnresolvedPixiRecord>,
}

/// A fully-resolved source record with all metadata available.
///
/// This is the primary type used throughout the codebase for building,
/// installing, and solving. Re-exported as `SourceRecord` from the crate root.
pub type FullSourceRecord = SourceRecord<FullSourceRecordData>;

/// A source record with only minimal metadata (name, depends, sources).
///
/// Produced when a mutable (path-based) source is written to the lock file.
/// Not used directly outside this crate; see [`UnresolvedSourceRecord`].
pub type PartialSourceRecord = SourceRecord<PartialSourceRecordData>;

/// A source record that may be either full or partial. This is the lock-file
/// boundary type.
///
/// Use [`UnresolvedPixiRecord::try_into_resolved`](crate::UnresolvedPixiRecord::try_into_resolved)
/// to check whether resolution is needed.
pub type UnresolvedSourceRecord = SourceRecord<SourceRecordData>;

/// Minimal metadata for a source package whose full record is not yet known.
///
/// This is what gets stored in the lock file for mutable (path-based) sources,
/// since their full metadata (version, build string, etc.) can change between
/// runs and would be stale.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PartialSourceRecordData {
    /// The package name of the source record.
    pub name: PackageName,

    /// Dependencies on other packages (run-time requirements).
    pub depends: Vec<String>,

    /// Run-time constraints on co-installed packages.
    pub constrains: Vec<String>,

    /// Additional dependencies grouped by an extra/feature key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub experimental_extra_depends: BTreeMap<String, Vec<String>>,

    /// Variant-selection flags declared by the recipe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,

    /// PURLs (Package URLs) describing this package in other ecosystems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purls: Option<BTreeSet<PackageUrl>>,

    /// License of the package (SPDX expression or free-form string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Run-exports declared by the recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_exports: Option<RunExportsJson>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: BTreeMap<String, SourceLocationSpec>,
}

/// Complete metadata for a fully-evaluated source package.
///
/// Contains the full [`PackageRecord`] (version, build, dependencies, etc.)
/// plus the source dependency map.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FullSourceRecordData {
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: BTreeMap<String, SourceLocationSpec>,
}

/// Runtime-checked variant used at the lock-file boundary.
///
/// After reading a lock file, source records may be either full (immutable
/// sources like git) or partial (mutable sources like local paths). This enum
/// captures both cases and is resolved to [`FullSourceRecordData`] at startup.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
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

    /// Returns true if either the manifest source or build source is mutable
    /// (i.e. path-based and may change over time).
    pub fn has_mutable_source(&self) -> bool {
        self.manifest_source().is_mutable()
            || self
                .build_source()
                .as_ref()
                .is_some_and(|bs| bs.pinned().is_mutable())
    }

    /// Transform the data payload while preserving all shared fields.
    ///
    /// Useful for state transitions (e.g. Full → Unresolved, Partial → Full)
    /// without field-by-field reconstruction.
    pub fn map_data<D2>(self, f: impl FnOnce(D) -> D2) -> SourceRecord<D2> {
        SourceRecord {
            data: f(self.data),
            manifest_source: self.manifest_source,
            build_source: self.build_source,
            variants: self.variants,
            identifier_hash: self.identifier_hash,
            build_packages: self.build_packages,
            host_packages: self.host_packages,
        }
    }

    /// Fallible version of [`map_data`](Self::map_data).
    ///
    /// On success the record carries the new data type; on failure the record
    /// is reassembled with the error type so no information is lost.
    #[allow(clippy::result_large_err)]
    pub fn try_map_data<T, E>(
        self,
        f: impl FnOnce(D) -> Result<T, E>,
    ) -> Result<SourceRecord<T>, SourceRecord<E>> {
        let SourceRecord {
            data,
            manifest_source,
            build_source,
            variants,
            identifier_hash,
            build_packages,
            host_packages,
        } = self;
        match f(data) {
            Ok(new_data) => Ok(SourceRecord {
                data: new_data,
                manifest_source,
                build_source,
                variants,
                identifier_hash,
                build_packages,
                host_packages,
            }),
            Err(err_data) => Err(SourceRecord {
                data: err_data,
                manifest_source,
                build_source,
                variants,
                identifier_hash,
                build_packages,
                host_packages,
            }),
        }
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
    pub fn sources(&self) -> &BTreeMap<String, SourceLocationSpec> {
        &self.data.sources
    }

    /// Convert into lock-file compatible `CondaSourceData`.
    ///
    /// If either source (manifest or build) is mutable (path-based), the
    /// record is downgraded to partial metadata. This is intentional: mutable
    /// sources can change between runs, so storing full metadata (version,
    /// build string, hashes) would be misleading because it would appear locked
    /// but could silently become stale. By keeping only name, depends, and
    /// sources, we force re-evaluation at the next lock-file read.
    pub fn into_conda_source_data(self, workspace_root: &Path) -> CondaSourceData {
        let has_mutable = self.has_mutable_source();
        let mut unresolved = SourceRecord::<SourceRecordData>::from(self);
        if has_mutable {
            // Downgrade full data to partial: keep name, depends, constrains,
            // and sources. Version/build/etc are dropped because they would go
            // stale on the next mutable-source rebuild.
            if let SourceRecordData::Full(full) = unresolved.data {
                unresolved.data = SourceRecordData::Partial(PartialSourceRecordData {
                    name: full.package_record.name,
                    depends: full.package_record.depends,
                    constrains: full.package_record.constrains,
                    experimental_extra_depends: full.package_record.experimental_extra_depends,
                    flags: full.package_record.flags,
                    purls: full.package_record.purls,
                    license: full.package_record.license,
                    run_exports: full.package_record.run_exports,
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

    /// Run-time constraints.
    pub fn constrains(&self) -> &[String] {
        &self.data.constrains
    }

    /// Source dependency locations.
    pub fn sources(&self) -> &BTreeMap<String, SourceLocationSpec> {
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

    /// Run-time constraints.
    pub fn constrains(&self) -> &[String] {
        match &self.data {
            SourceRecordData::Full(full) => &full.package_record.constrains,
            SourceRecordData::Partial(partial) => &partial.constrains,
        }
    }

    /// Source dependency locations.
    pub fn sources(&self) -> &BTreeMap<String, SourceLocationSpec> {
        match &self.data {
            SourceRecordData::Full(full) => &full.sources,
            SourceRecordData::Partial(partial) => &partial.sources,
        }
    }

    /// Convert into lock-file compatible `CondaSourceData<SourceMetadata>`.
    ///
    /// If the source is mutable (path-based), full metadata is downgraded to
    /// partial so the lock file does not store stale version/build data.
    pub fn into_conda_source_data(self, _workspace_root: &Path) -> CondaSourceData {
        // Downgrade full records to partial when the source is mutable.
        let is_mutable = self.manifest_source.is_mutable()
            || self
                .build_source
                .as_ref()
                .is_some_and(|bs| bs.pinned().is_mutable());

        let package_build_source = build_source_to_package_build_source(self.build_source);

        let data = if is_mutable {
            match self.data {
                SourceRecordData::Full(full) => {
                    SourceRecordData::Partial(PartialSourceRecordData {
                        name: full.package_record.name,
                        depends: full.package_record.depends,
                        constrains: full.package_record.constrains,
                        experimental_extra_depends: full.package_record.experimental_extra_depends,
                        flags: full.package_record.flags,
                        purls: full.package_record.purls,
                        license: full.package_record.license,
                        run_exports: full.package_record.run_exports,
                        sources: full.sources,
                    })
                }
                partial @ SourceRecordData::Partial(_) => partial,
            }
        } else {
            self.data
        };

        let (metadata, sources) = match data {
            SourceRecordData::Full(full) => (
                SourceMetadata::Full(Box::new(full.package_record)),
                full.sources,
            ),
            SourceRecordData::Partial(partial) => (
                SourceMetadata::Partial(Box::new(PartialSourceMetadata {
                    name: partial.name,
                    depends: partial.depends,
                    constrains: partial.constrains,
                    experimental_extra_depends: partial.experimental_extra_depends,
                    flags: partial.flags,
                    license: partial.license,
                    purls: partial.purls,
                    run_exports: partial.run_exports,
                })),
                partial.sources,
            ),
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
            sources: sources.into_iter().map(|(k, v)| (k, v.into())).collect(),
            source_data: SourceData::default(),
            metadata,
        }
    }

    /// Create from lock-file `CondaSourceData<SourceMetadata>`.
    ///
    /// `build_packages` and `host_packages` must be resolved by the caller
    /// from the lock file's package table, since `CondaSourceData` only
    /// carries index sets and the full list of packages is not available
    /// here.
    pub fn from_conda_source_data(
        data: CondaSourceData,
        _workspace_root: &std::path::Path,
        build_packages: Vec<crate::UnresolvedPixiRecord>,
        host_packages: Vec<crate::UnresolvedPixiRecord>,
    ) -> Result<Self, ParseLockFileError> {
        let manifest_source: PinnedSourceSpec = data.location.try_into()?;
        let build_source =
            package_build_source_to_build_source(data.package_build_source, &manifest_source)?;

        let sources = data
            .sources
            .into_iter()
            .map(|(k, v)| (k, SourceLocationSpec::from(v)))
            .collect();

        let record_data = match data.metadata {
            SourceMetadata::Full(package_record) => SourceRecordData::Full(FullSourceRecordData {
                package_record: package_record.as_ref().clone(),
                sources,
            }),
            SourceMetadata::Partial(partial) => {
                SourceRecordData::Partial(PartialSourceRecordData {
                    name: partial.name,
                    depends: partial.depends,
                    constrains: partial.constrains,
                    experimental_extra_depends: partial.experimental_extra_depends,
                    flags: partial.flags,
                    purls: partial.purls,
                    license: partial.license,
                    run_exports: partial.run_exports,
                    sources,
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
            build_packages,
            host_packages,
        })
    }
}

/// Upcast from full to unresolved.
impl From<SourceRecord<FullSourceRecordData>> for SourceRecord<SourceRecordData> {
    fn from(record: SourceRecord<FullSourceRecordData>) -> Self {
        record.map_data(SourceRecordData::Full)
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
                    Vec::new(),
                    Vec::new(),
                )
                .expect("from_conda_source_data should succeed");
                match unresolved.data {
                    SourceRecordData::Full(full) => super::SourceRecord {
                        data: full,
                        manifest_source: unresolved.manifest_source,
                        build_source: unresolved.build_source,
                        variants: unresolved.variants,
                        identifier_hash: unresolved.identifier_hash,
                        build_packages: unresolved.build_packages,
                        host_packages: unresolved.host_packages,
                    },
                    SourceRecordData::Partial(_) => {
                        panic!("fixture should only contain full source records")
                    }
                }
            })
            .collect();

        // Write back: mutable (path) records should become partial,
        // immutable (git) records stay full.
        let roundtrip_lock = build_lock_from_records(&roundtrip_records, workspace_root);
        let mut settings = insta::Settings::clone_current();
        settings.set_sort_maps(true);
        settings.bind(|| {
            insta::assert_snapshot!(roundtrip_lock);
        });
    }

    /// Round-trip a lock file whose conda source package carries build and
    /// host package handles. Uses [`LockFileResolver`] to rebuild records
    /// with their build/host packages populated from the lockfile's package
    /// table, then writes them back via [`UnresolvedPixiRecord::into_conda_package_data`]
    /// — the path that should serialize the handles again. A diff against
    /// the fixture fails if the round-trip drops build/host information.
    #[test]
    fn roundtrip_source_records_with_build_host_packages() {
        assert_roundtrip_identity("source_with_build_host_packages.lock");
    }

    /// Same contract as [`roundtrip_source_records_with_build_host_packages`],
    /// but the outer source has another source in its `build_packages`. This
    /// exercises the recursive branch of `register_source_deps` — the only
    /// path that actually fires when a source depends on a source at build
    /// time.
    #[test]
    fn roundtrip_source_records_with_nested_source_build_dep() {
        assert_roundtrip_identity("nested_source_build_dep.lock");
    }

    fn assert_roundtrip_identity(fixture_name: &str) {
        use crate::LockFileResolver;

        let workspace_root = Path::new("/workspace");

        let lock_source = fixture_source(fixture_name);
        let lock_file = LockFile::from_str_with_base_directory(&lock_source, Some(workspace_root))
            .expect("failed to load lock file fixture");

        let resolver =
            LockFileResolver::build(&lock_file, workspace_root).expect("resolver should build");

        let environment = lock_file
            .default_environment()
            .expect("expected default environment");

        let mut builder = LockFileBuilder::new()
            .with_platforms(
                lock_file
                    .platforms()
                    .map(|p| rattler_lock::PlatformData {
                        name: p.name().clone(),
                        subdir: p.subdir(),
                        virtual_packages: p.virtual_packages().to_vec(),
                    })
                    .collect(),
            )
            .expect("platforms should be unique");
        builder.set_channels(
            DEFAULT_ENVIRONMENT_NAME,
            [Channel::from("https://conda.anaconda.org/conda-forge/")],
        );

        for (platform, packages) in environment.packages_by_platform() {
            let platform_str = platform.subdir().to_string();
            for package in packages {
                let Some(record) = resolver.get_for_package(package) else {
                    continue;
                };
                let data = record.into_conda_package_data(&mut builder, workspace_root);
                builder
                    .add_conda_package(DEFAULT_ENVIRONMENT_NAME, &platform_str, data)
                    .expect("platform was registered");
            }
        }

        let roundtrip_lock = builder
            .finish()
            .render_to_string()
            .expect("failed to render lock file");

        assert_eq!(
            roundtrip_lock.trim_end().replace("\r\n", "\n"),
            lock_source.trim_end().replace("\r\n", "\n"),
            "round-trip of {fixture_name} through LockFileResolver + into_conda_package_data should be identity"
        );
    }

    /// Load the lock file body from a static fixture file with full metadata.
    fn lock_source_from_fixture() -> String {
        fixture_source("full_source_records.lock")
    }

    fn fixture_source(name: &str) -> String {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/test_fixtures")
            .join(name);
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
                constrains: Vec::new(),
                experimental_extra_depends: BTreeMap::new(),
                flags: vec![],
                purls: None,
                license: None,
                run_exports: None,
                sources: BTreeMap::new(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: typed_path::Utf8TypedPathBuf::from("./my-package"),
            }),
            build_source: None,
            variants: BTreeMap::from([(
                "python".into(),
                crate::VariantValue::from("3.12".to_string()),
            )]),
            identifier_hash: Some("abcd1234".to_string()),
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        };

        assert_eq!(partial.name().as_source(), "my-package");

        // Roundtrip through CondaSourceData.
        let conda_data = partial.into_conda_source_data(workspace_root);
        let roundtripped = super::SourceRecord::<SourceRecordData>::from_conda_source_data(
            conda_data,
            workspace_root,
            Vec::new(),
            Vec::new(),
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

        let unresolved = UnresolvedPixiRecord::from_conda_package_data(
            CondaPackageData::Source(Box::new(conda_source)),
            workspace_root,
            Vec::new(),
            Vec::new(),
        )
        .expect("from_conda_package_data should succeed");

        let resolved = unresolved.try_into_resolved();
        assert!(resolved.is_ok());
        assert!(matches!(resolved.unwrap(), PixiRecord::Source(_)));
    }

    #[test]
    fn try_into_resolved_with_partial_record() {
        use crate::{PinnedPathSpec, PinnedSourceSpec, UnresolvedPixiRecord};
        use std::sync::Arc;

        let partial =
            UnresolvedPixiRecord::Source(Arc::new(super::SourceRecord::<SourceRecordData> {
                data: SourceRecordData::Partial(PartialSourceRecordData {
                    name: PackageName::from_str("partial-pkg").unwrap(),
                    depends: vec![],
                    constrains: vec![],
                    experimental_extra_depends: BTreeMap::new(),
                    flags: vec![],
                    purls: None,
                    license: None,
                    run_exports: None,
                    sources: BTreeMap::new(),
                }),
                manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                    path: typed_path::Utf8TypedPathBuf::from("./partial-pkg"),
                }),
                build_source: None,
                variants: BTreeMap::new(),
                identifier_hash: None,
                build_packages: Vec::new(),
                host_packages: Vec::new(),
            }));

        let result = partial.try_into_resolved();
        assert!(result.is_err());
        let still_partial = result.unwrap_err();
        assert_eq!(still_partial.name().as_source(), "partial-pkg");
    }

    /// Helper to create a minimal full source record for testing.
    fn make_full_record(
        name: &str,
        manifest_source: PinnedSourceSpec,
        build_source: Option<PinnedBuildSourceSpec>,
        variants: BTreeMap<String, crate::VariantValue>,
    ) -> SourceRecord {
        let mut record = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h1234_0".into(),
        );
        record.subdir = "linux-64".into();
        record.depends = vec!["python >=3.8".into()];
        SourceRecord {
            data: FullSourceRecordData {
                package_record: record,
                sources: BTreeMap::new(),
            },
            manifest_source,
            build_source,
            variants,
            identifier_hash: None,
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        }
    }

    fn path_source(p: &str) -> PinnedSourceSpec {
        PinnedSourceSpec::Path(PinnedPathSpec {
            path: typed_path::Utf8TypedPathBuf::from(p),
        })
    }

    fn git_source() -> PinnedSourceSpec {
        PinnedSourceSpec::Git(crate::PinnedGitSpec {
            git: url::Url::parse("https://github.com/example/repo.git").unwrap(),
            source: crate::PinnedGitCheckout {
                commit: pixi_git::sha::GitSha::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                    .unwrap(),
                subdirectory: Default::default(),
                reference: pixi_spec::GitReference::DefaultBranch,
            },
        })
    }

    #[test]
    fn path_source_is_mutable() {
        let record = make_full_record("my-pkg", path_source("./my-pkg"), None, BTreeMap::new());
        assert!(record.has_mutable_source());
    }

    #[test]
    fn git_source_is_not_mutable() {
        let record = make_full_record("my-pkg", git_source(), None, BTreeMap::new());
        assert!(!record.has_mutable_source());
    }

    #[test]
    fn mutable_build_source_triggers_mutable() {
        let record = make_full_record(
            "my-pkg",
            git_source(),
            Some(PinnedBuildSourceSpec::Absolute(path_source("./build-dir"))),
            BTreeMap::new(),
        );
        assert!(record.has_mutable_source());
    }

    #[test]
    fn path_source_downgrades_to_partial_in_lockfile() {
        let record = make_full_record("my-pkg", path_source("./my-pkg"), None, BTreeMap::new());
        let conda_data = record.into_conda_source_data(Path::new("/workspace"));
        assert!(
            matches!(conda_data.metadata, SourceMetadata::Partial(_)),
            "mutable source should be downgraded to partial"
        );
    }

    #[test]
    fn git_source_stays_full_in_lockfile() {
        let record = make_full_record("my-pkg", git_source(), None, BTreeMap::new());
        let conda_data = record.into_conda_source_data(Path::new("/workspace"));
        assert!(
            matches!(conda_data.metadata, SourceMetadata::Full(_)),
            "immutable source should stay full"
        );
    }

    #[test]
    fn refers_to_same_output_same_name_same_variants() {
        let variants = BTreeMap::from([(
            "python".into(),
            crate::VariantValue::from("3.12".to_string()),
        )]);
        let a = make_full_record("pkg", path_source("."), None, variants.clone());
        let b = make_full_record("pkg", path_source("."), None, variants);
        assert!(a.refers_to_same_output(&b));
    }

    #[test]
    fn refers_to_same_output_different_variants() {
        let a = make_full_record(
            "pkg",
            path_source("."),
            None,
            BTreeMap::from([(
                "python".into(),
                crate::VariantValue::from("3.12".to_string()),
            )]),
        );
        let b = make_full_record(
            "pkg",
            path_source("."),
            None,
            BTreeMap::from([(
                "python".into(),
                crate::VariantValue::from("3.11".to_string()),
            )]),
        );
        assert!(!a.refers_to_same_output(&b));
    }

    #[test]
    fn refers_to_same_output_empty_variants_is_true() {
        let a = make_full_record("pkg", path_source("."), None, BTreeMap::new());
        let b = make_full_record(
            "pkg",
            path_source("."),
            None,
            BTreeMap::from([(
                "python".into(),
                crate::VariantValue::from("3.12".to_string()),
            )]),
        );
        assert!(a.refers_to_same_output(&b));
    }

    #[test]
    fn refers_to_same_output_different_names() {
        let variants = BTreeMap::from([(
            "python".into(),
            crate::VariantValue::from("3.12".to_string()),
        )]);
        let a = make_full_record("pkg-a", path_source("."), None, variants.clone());
        let b = make_full_record("pkg-b", path_source("."), None, variants);
        assert!(!a.refers_to_same_output(&b));
    }

    #[test]
    fn map_data_preserves_shared_fields() {
        let record = make_full_record(
            "my-pkg",
            path_source("./my-pkg"),
            None,
            BTreeMap::from([(
                "python".into(),
                crate::VariantValue::from("3.12".to_string()),
            )]),
        );
        let unresolved: super::SourceRecord<SourceRecordData> =
            record.map_data(SourceRecordData::Full);
        assert_eq!(unresolved.name().as_source(), "my-pkg");
        assert!(unresolved.data.is_full());
        assert_eq!(
            unresolved.variants.get("python").map(|v| v.to_string()),
            Some("3.12".to_string())
        );
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
        let unresolved = super::SourceRecord::<SourceRecordData>::from_conda_source_data(
            conda_source,
            workspace_root,
            Vec::new(),
            Vec::new(),
        )
        .expect("from_conda_source_data should succeed");
        assert!(unresolved.data.is_full());

        // Roundtrip through CondaSourceData.
        let conda_data = unresolved.into_conda_source_data(workspace_root);
        let roundtripped = super::SourceRecord::<SourceRecordData>::from_conda_source_data(
            conda_data,
            workspace_root,
            Vec::new(),
            Vec::new(),
        )
        .expect("roundtrip should succeed");

        assert!(roundtripped.data.is_full());
    }
}
