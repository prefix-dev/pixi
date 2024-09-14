mod pinned_source;
mod source_record;
use rattler_conda_types::{
    package::ArchiveIdentifier, MatchSpec, Matches, NamelessMatchSpec, PackageRecord,
    RepoDataRecord,
};
use rattler_lock::{CondaPackageData, ConversionError, UrlOrPath};
use thiserror::Error;

pub use pinned_source::{
    ParseError, PinnedGitSpec, PinnedPathSpec, PinnedSourceSpec, PinnedUrlSpec,
};
pub use source_record::SourceRecord;

/// A record of a conda package that is either something installable from a
/// binary file or something that still requires building.
///
/// This is basically a superset of a regular [`RepoDataRecord`].
#[derive(Debug, Clone)]
pub enum PixiRecord {
    Binary(RepoDataRecord),
    Source(SourceRecord),
}
impl PixiRecord {
    /// Metadata information of the package.
    pub fn package_record(&self) -> &PackageRecord {
        match self {
            PixiRecord::Binary(record) => &record.package_record,
            PixiRecord::Source(record) => &record.package_record,
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

    /// Converts this instance into a source record if it is a source record.
    pub fn into_source(self) -> Option<SourceRecord> {
        match self {
            PixiRecord::Binary(_) => None,
            PixiRecord::Source(record) => Some(record),
        }
    }

    /// Returns a mutable reference to the source record if it is a source
    /// record.
    pub fn as_source_mut(&mut self) -> Option<&mut SourceRecord> {
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
        let archive_identifier = value
            .location
            .file_name()
            .and_then(ArchiveIdentifier::try_from_filename);
        if archive_identifier.is_some() {
            let location = value.location.clone();
            Ok(PixiRecord::Binary(value.try_into().map_err(
                |err| match err {
                    ConversionError::Missing(field) => ParseLockFileError::Missing(location, field),
                    ConversionError::LocationToUrlConversionError(err) => {
                        ParseLockFileError::InvalidRecordUrl(location, err)
                    }
                },
            )?))
        } else {
            Ok(PixiRecord::Source(value.try_into()?))
        }
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
