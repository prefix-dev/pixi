mod canonical_spec;
mod dev_source_record;
mod pinned_source;
mod source_record;

pub use canonical_spec::{CanonicalGit, CanonicalPath, CanonicalSourceLocation, CanonicalUrl};
pub use dev_source_record::DevSourceRecord;

use std::path::Path;

pub use pinned_source::{
    LockedGitUrl, MutablePinnedSourceSpec, ParseError, PinnedGitCheckout, PinnedGitSpec,
    PinnedPathSpec, PinnedSourceSpec, PinnedUrlSpec, SourceMismatchError,
};
pub use pixi_variant::VariantValue;
use rattler_conda_types::{
    MatchSpec, Matches, NamelessMatchSpec, PackageName, PackageRecord, RepoDataRecord,
};
use rattler_lock::{CondaPackageData, ConversionError, UrlOrPath};
use serde::Serialize;
pub use source_record::{
    FullSourceRecord as SourceRecord, FullSourceRecordData, PartialSourceRecord,
    PartialSourceRecordData, PinnedBuildSourceSpec, SourceRecordData, UnresolvedSourceRecord,
};
use thiserror::Error;

/// A record of a conda package that is either something installable from a
/// binary file or something that still requires building.
///
/// This is basically a superset of a regular [`RepoDataRecord`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum PixiRecord {
    Binary(RepoDataRecord),
    Source(SourceRecord),
}
impl PixiRecord {
    /// The name of the package
    pub fn name(&self) -> &PackageName {
        &self.package_record().name
    }

    /// Metadata information of the package.
    pub fn package_record(&self) -> &PackageRecord {
        match self {
            PixiRecord::Binary(record) => &record.package_record,
            PixiRecord::Source(record) => &record.data.package_record,
        }
    }

    /// Convert to CondaPackageData with paths made relative to workspace_root.
    /// This should be used when writing to the lock file.
    pub fn into_conda_package_data(self, workspace_root: &Path) -> CondaPackageData {
        match self {
            PixiRecord::Binary(record) => record.into(),
            PixiRecord::Source(record) => {
                CondaPackageData::Source(record.into_conda_source_data(workspace_root))
            }
        }
    }

    /// Returns a reference to the binary record if it is a binary record.
    pub fn as_binary(&self) -> Option<&RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    /// Converts this instance into a binary record if it is a binary record.
    pub fn into_binary(self) -> Option<RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    /// Converts this instance into a source record if it is a source
    pub fn into_source(self) -> Option<SourceRecord> {
        match self {
            PixiRecord::Binary(_) => None,
            PixiRecord::Source(record) => Some(record),
        }
    }

    /// Returns a mutable reference to the binary record if it is a binary
    /// record.
    pub fn as_binary_mut(&mut self) -> Option<&mut RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    /// Returns the source record if it is a source record.
    pub fn as_source(&self) -> Option<&SourceRecord> {
        match self {
            PixiRecord::Binary(_) => None,
            PixiRecord::Source(record) => Some(record),
        }
    }
}

impl From<SourceRecord> for PixiRecord {
    fn from(value: SourceRecord) -> Self {
        PixiRecord::Source(value)
    }
}

impl From<RepoDataRecord> for PixiRecord {
    fn from(value: RepoDataRecord) -> Self {
        PixiRecord::Binary(value)
    }
}

/// A record that may contain partial source metadata (not yet resolved).
///
/// Used at the lock-file boundary: lock-file read produces these, and they must
/// be resolved to [`PixiRecord`] before use in solving/installing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum UnresolvedPixiRecord {
    Binary(RepoDataRecord),
    Source(UnresolvedSourceRecord),
}

impl UnresolvedPixiRecord {
    /// The name of the package.
    pub fn name(&self) -> &PackageName {
        match self {
            UnresolvedPixiRecord::Binary(record) => &record.package_record.name,
            UnresolvedPixiRecord::Source(record) => record.name(),
        }
    }

    /// Run-time dependencies.
    pub fn depends(&self) -> &[String] {
        match self {
            UnresolvedPixiRecord::Binary(record) => &record.package_record.depends,
            UnresolvedPixiRecord::Source(record) => record.depends(),
        }
    }

    /// Source dependency locations. Empty for binary records.
    pub fn sources(&self) -> &std::collections::HashMap<String, pixi_spec::SourceLocationSpec> {
        static EMPTY: std::sync::LazyLock<
            std::collections::HashMap<String, pixi_spec::SourceLocationSpec>,
        > = std::sync::LazyLock::new(std::collections::HashMap::new);
        match self {
            UnresolvedPixiRecord::Binary(_) => &EMPTY,
            UnresolvedPixiRecord::Source(record) => record.sources(),
        }
    }

    /// Returns a reference to the binary record if it is one.
    pub fn as_binary(&self) -> Option<&RepoDataRecord> {
        match self {
            UnresolvedPixiRecord::Binary(record) => Some(record),
            UnresolvedPixiRecord::Source(_) => None,
        }
    }

    /// Returns a reference to the source record if it is one.
    pub fn as_source(&self) -> Option<&UnresolvedSourceRecord> {
        match self {
            UnresolvedPixiRecord::Binary(_) => None,
            UnresolvedPixiRecord::Source(record) => Some(record),
        }
    }

    /// Returns the full package record if available (binary or full source).
    pub fn package_record(&self) -> Option<&PackageRecord> {
        match self {
            UnresolvedPixiRecord::Binary(record) => Some(&record.package_record),
            UnresolvedPixiRecord::Source(record) => match &record.data {
                SourceRecordData::Full(full) => Some(&full.package_record),
                SourceRecordData::Partial(_) => None,
            },
        }
    }

    /// Returns true if this is a partial source record.
    pub fn is_partial(&self) -> bool {
        matches!(
            self,
            UnresolvedPixiRecord::Source(s) if s.data.is_partial()
        )
    }

    /// Create from lock-file `CondaPackageData`.
    pub fn from_conda_package_data(
        data: CondaPackageData,
        workspace_root: &std::path::Path,
    ) -> Result<Self, ParseLockFileError> {
        match data {
            CondaPackageData::Binary(value) => {
                let location = value.location.clone();
                Ok(UnresolvedPixiRecord::Binary(value.try_into().map_err(
                    |err| match err {
                        ConversionError::Missing(field) => {
                            ParseLockFileError::Missing(location, field)
                        }
                        ConversionError::LocationToUrlConversionError(err) => {
                            ParseLockFileError::InvalidRecordUrl(location, err)
                        }
                        ConversionError::InvalidBinaryPackageLocation => {
                            ParseLockFileError::InvalidArchiveFilename(location)
                        }
                    },
                )?))
            }
            CondaPackageData::Source(value) => Ok(UnresolvedPixiRecord::Source(
                UnresolvedSourceRecord::from_conda_source_data(value, workspace_root)?,
            )),
        }
    }

    /// Convert to `CondaPackageData` for lock-file write.
    pub fn into_conda_package_data(self, workspace_root: &Path) -> CondaPackageData {
        match self {
            UnresolvedPixiRecord::Binary(record) => record.into(),
            UnresolvedPixiRecord::Source(record) => {
                CondaPackageData::Source(record.into_conda_source_data(workspace_root))
            }
        }
    }

    /// Try to convert into a fully resolved `PixiRecord`.
    ///
    /// Returns `Ok(PixiRecord)` if this is a binary record or a full source
    /// record. Returns `Err(self)` if this is a partial source record.
    pub fn try_into_resolved(self) -> Result<PixiRecord, Self> {
        match self {
            UnresolvedPixiRecord::Binary(record) => Ok(PixiRecord::Binary(record)),
            UnresolvedPixiRecord::Source(source) => match source.data {
                SourceRecordData::Full(full) => {
                    Ok(PixiRecord::Source(source_record::SourceRecord {
                        data: full,
                        manifest_source: source.manifest_source,
                        build_source: source.build_source,
                        variants: source.variants,
                        identifier_hash: source.identifier_hash,
                    }))
                }
                SourceRecordData::Partial(partial) => {
                    Err(UnresolvedPixiRecord::Source(source_record::SourceRecord {
                        data: SourceRecordData::Partial(partial),
                        manifest_source: source.manifest_source,
                        build_source: source.build_source,
                        variants: source.variants,
                        identifier_hash: source.identifier_hash,
                    }))
                }
            },
        }
    }
}

impl From<PixiRecord> for UnresolvedPixiRecord {
    fn from(record: PixiRecord) -> Self {
        match record {
            PixiRecord::Binary(r) => UnresolvedPixiRecord::Binary(r),
            PixiRecord::Source(r) => UnresolvedPixiRecord::Source(r.into()),
        }
    }
}

#[derive(Debug, Error)]
pub enum ParseLockFileError {
    #[error("missing field/fields '{1}' for package {0}")]
    Missing(UrlOrPath, String),

    #[error("Invalid archive file name for package {0}")]
    InvalidArchiveFilename(UrlOrPath),

    #[error("invalid url for package {0}")]
    InvalidRecordUrl(UrlOrPath, #[source] file_url::FileURLParseError),

    #[error(transparent)]
    PinnedSourceSpecError(#[from] pinned_source::ParseError),
}

impl Matches<PixiRecord> for NamelessMatchSpec {
    fn matches(&self, record: &PixiRecord) -> bool {
        match record {
            PixiRecord::Binary(record) => self.matches(record),
            PixiRecord::Source(record) => self.matches(record),
        }
    }
}

impl Matches<PixiRecord> for MatchSpec {
    fn matches(&self, record: &PixiRecord) -> bool {
        match record {
            PixiRecord::Binary(record) => self.matches(record),
            PixiRecord::Source(record) => self.matches(record),
        }
    }
}

impl AsRef<PackageRecord> for PixiRecord {
    fn as_ref(&self) -> &PackageRecord {
        match self {
            PixiRecord::Binary(record) => record.as_ref(),
            PixiRecord::Source(record) => record.as_ref(),
        }
    }
}
