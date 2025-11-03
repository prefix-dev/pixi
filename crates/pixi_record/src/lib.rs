mod pinned_source;
mod source_record;

pub use pinned_source::{
    LockedGitUrl, MutablePinnedSourceSpec, ParseError, PinnedGitCheckout, PinnedGitSpec,
    PinnedPathSpec, PinnedSourceSpec, PinnedUrlSpec, SourceMismatchError,
};
use rattler_conda_types::{
    MatchSpec, Matches, PackageName, PackageRecord, RepoDataRecord, VersionWithSource,
    package::RunExportsJson,
};
use rattler_lock::{CondaPackageData, ConversionError, UrlOrPath};
use serde::Serialize;
pub use source_record::{InputHash, SourcePackageRecord, SourceRecord};
// Re-export VariantValue for convenience
pub use rattler_lock::VariantValue;

/// A map of variant keys to their selected values.
/// Used to uniquely identify which output variant of a source package to build.
pub type SelectedVariant = std::collections::BTreeMap<String, VariantValue>;
use thiserror::Error;

/// A record of a conda package that is either something installable from a
/// binary file or something that still requires building.
///
/// This is basically a superset of a regular [`RepoDataRecord`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum PixiRecord {
    Binary(RepoDataRecord),
    Source(SourceRecord),
}
impl PixiRecord {
    /// The name of the package
    pub fn name(&self) -> &PackageName {
        match self {
            PixiRecord::Binary(record) => &record.package_record.name,
            PixiRecord::Source(record) => &record.name,
        }
    }

    /// Returns the version of the package if it is known.
    pub fn version(&self) -> Option<&VersionWithSource> {
        match self {
            PixiRecord::Binary(record) => Some(&record.package_record.version),
            PixiRecord::Source(record) => record.version.as_ref(),
        }
    }

    /// The dependencies of the package
    pub fn depends(&self) -> &[String] {
        match self {
            PixiRecord::Binary(record) => &record.package_record.depends,
            PixiRecord::Source(record) => &record.depends,
        }
    }

    /// The constraints of the package
    pub fn constrains(&self) -> &[String] {
        match self {
            PixiRecord::Binary(record) => &record.package_record.constrains,
            PixiRecord::Source(record) => &record.constrains,
        }
    }

    /// Metadata information of the package.
    ///
    /// Returns `Some` for binary packages, `None` for source packages.
    /// Source packages don't have version/build information in the new lock format.
    pub fn package_record(&self) -> Option<&PackageRecord> {
        match self {
            PixiRecord::Binary(record) => Some(&record.package_record),
            PixiRecord::Source(_) => None,
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

#[derive(Debug, Error)]
pub enum ParseLockFileError {
    #[error("missing field/fields '{1}' for package {0}")]
    Missing(UrlOrPath, String),

    #[error("invalid url for package {0}")]
    InvalidRecordUrl(UrlOrPath, #[source] file_url::FileURLParseError),

    #[error(transparent)]
    PinnedSourceSpecError(#[from] pinned_source::ParseError),
}

impl TryFrom<CondaPackageData> for PixiRecord {
    type Error = ParseLockFileError;

    fn try_from(value: CondaPackageData) -> Result<Self, Self::Error> {
        let record = match value {
            CondaPackageData::Binary(value) => {
                let location = value.location.clone();
                PixiRecord::Binary(value.try_into().map_err(|err| match err {
                    ConversionError::Missing(field) => ParseLockFileError::Missing(location, field),
                    ConversionError::LocationToUrlConversionError(err) => {
                        ParseLockFileError::InvalidRecordUrl(location, err)
                    }
                })?)
            }
            CondaPackageData::Source(value) => PixiRecord::Source(value.try_into()?),
        };
        Ok(record)
    }
}

impl From<PixiRecord> for CondaPackageData {
    fn from(value: PixiRecord) -> Self {
        match value {
            PixiRecord::Binary(record) => record.into(),
            PixiRecord::Source(record) => record.into(),
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

/// A conda package record with complete metadata for both binary and source packages.
///
/// This is the fully-resolved version of [`PixiRecord`], where source packages include
/// complete package metadata (version, build, timestamp, etc.) via [`SourcePackageRecord`].
///
/// Unlike [`PixiRecord`], this type contains all information needed for dependency solving.
/// It is used during solve operations but **not** stored in lock files (only [`PixiRecord`]
/// is persisted).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum PixiPackageRecord {
    Binary(RepoDataRecord),
    Source(SourcePackageRecord),
}

impl PixiPackageRecord {
    /// The name of the package
    pub fn name(&self) -> &PackageName {
        match self {
            PixiPackageRecord::Binary(record) => &record.package_record.name,
            PixiPackageRecord::Source(record) => &record.source_record.name,
        }
    }

    /// The version of the package
    pub fn version(&self) -> &VersionWithSource {
        match self {
            PixiPackageRecord::Binary(record) => &record.package_record.version,
            PixiPackageRecord::Source(record) => &record.version,
        }
    }

    pub fn run_exports(&self) -> Option<&RunExportsJson> {
        match self {
            PixiPackageRecord::Binary(record) => record.package_record.run_exports.as_ref(),
            PixiPackageRecord::Source(record) => record.run_exports.as_ref(),
        }
    }

    /// Returns a reference to the binary record if it is a binary record.
    pub fn as_binary(&self) -> Option<&RepoDataRecord> {
        match self {
            PixiPackageRecord::Binary(record) => Some(record),
            PixiPackageRecord::Source(_) => None,
        }
    }

    /// Returns a reference to the binary record if it is a binary record.
    pub fn as_binary_mut(&mut self) -> Option<&mut RepoDataRecord> {
        match self {
            PixiPackageRecord::Binary(record) => Some(record),
            PixiPackageRecord::Source(_) => None,
        }
    }

    /// Converts this instance into a binary record if it is a binary record.
    pub fn into_binary(self) -> Option<RepoDataRecord> {
        match self {
            PixiPackageRecord::Binary(record) => Some(record),
            PixiPackageRecord::Source(_) => None,
        }
    }

    /// Returns a reference to the source record if it is a source record.
    pub fn as_source(&self) -> Option<&SourcePackageRecord> {
        match self {
            PixiPackageRecord::Binary(_) => None,
            PixiPackageRecord::Source(record) => Some(record),
        }
    }

    /// Converts this instance into a source record if it is a source record.
    pub fn into_source(self) -> Option<SourcePackageRecord> {
        match self {
            PixiPackageRecord::Binary(_) => None,
            PixiPackageRecord::Source(record) => Some(record),
        }
    }

    /// Returns the package record.
    pub fn package_record(&self) -> PackageRecord {
        match self {
            PixiPackageRecord::Binary(record) => record.package_record.clone(),
            PixiPackageRecord::Source(record) => record.package_record(),
        }
    }
}

impl From<SourcePackageRecord> for PixiPackageRecord {
    fn from(value: SourcePackageRecord) -> Self {
        PixiPackageRecord::Source(value)
    }
}

impl From<RepoDataRecord> for PixiPackageRecord {
    fn from(value: RepoDataRecord) -> Self {
        PixiPackageRecord::Binary(value)
    }
}

impl From<PixiPackageRecord> for PixiRecord {
    fn from(value: PixiPackageRecord) -> Self {
        match value {
            PixiPackageRecord::Binary(record) => PixiRecord::Binary(record),
            PixiPackageRecord::Source(record) => PixiRecord::Source(record.into()),
        }
    }
}
