mod pinned_source;
mod source_record;

pub use pinned_source::{
    LockedGitUrl, MutablePinnedSourceSpec, ParseError, PinnedGitCheckout, PinnedGitSpec,
    PinnedPathSpec, PinnedSourceSpec, PinnedUrlSpec, SourceMismatchError,
};
use rattler_conda_types::{
    MatchSpec, Matches, NamelessMatchSpec, PackageName, PackageRecord, RepoDataRecord,
};
use rattler_lock::{CondaPackageData, ConversionError, UrlOrPath};
use serde::Serialize;
pub use source_record::{InputHash, SourceRecord};
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
            PixiRecord::Source(record) => &record.package_record,
        }
    }

    /// Convert to CondaPackageData with paths made relative to workspace_root.
    /// This should be used when writing to the lock file.
    pub fn into_conda_package_data(
        self,
        workspace_root: &std::path::Path,
    ) -> CondaPackageData {
        match self {
            PixiRecord::Binary(record) => record.into(),
            PixiRecord::Source(record) => {
                CondaPackageData::Source(record.into_conda_source_data(workspace_root))
            }
        }
    }

    /// Create PixiRecord from CondaPackageData with paths resolved relative to workspace_root.
    /// This should be used when reading from the lock file.
    pub fn from_conda_package_data(
        data: CondaPackageData,
        workspace_root: &std::path::Path,
    ) -> Result<Self, ParseLockFileError> {
        let record = match data {
            CondaPackageData::Binary(value) => {
                let location = value.location.clone();
                PixiRecord::Binary(value.try_into().map_err(|err| match err {
                    ConversionError::Missing(field) => ParseLockFileError::Missing(location, field),
                    ConversionError::LocationToUrlConversionError(err) => {
                        ParseLockFileError::InvalidRecordUrl(location, err)
                    }
                })?)
            }
            CondaPackageData::Source(value) => {
                PixiRecord::Source(SourceRecord::from_conda_source_data(value, workspace_root)?)
            }
        };
        Ok(record)
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
