use rattler_conda_types::{
    package::ArchiveIdentifier, MatchSpec, Matches, NamelessMatchSpec, PackageRecord,
    RepoDataRecord,
};
use rattler_lock::{CondaPackageData, ConversionError, UrlOrPath};
use thiserror::Error;
use crate::build::pinned::{PinnedPathSpec, PinnedSourceSpec};

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

    pub fn as_binary(&self) -> Option<&RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    pub fn into_binary(self) -> Option<RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    pub fn as_binary_mut(&mut self) -> Option<&mut RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    pub fn as_source(&self) -> Option<&SourceRecord> {
        match self {
            PixiRecord::Binary(_) => None,
            PixiRecord::Source(record) => Some(record),
        }
    }
}

/// A record of a conda package that still requires building.
#[derive(Debug, Clone)]
pub struct SourceRecord {
    /// Information about the conda package. This is metadata of the package
    /// after it has been build.
    pub package_record: PackageRecord,

    /// Exact definition of the source of the package.
    pub source: PinnedSourceSpec,
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

impl From<SourceRecord> for CondaPackageData {
    fn from(value: SourceRecord) -> Self {
        CondaPackageData {
            package_record: value.package_record,
            location: value.source.into(),
            file_name: None,
            channel: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ParseLockFileError {
    #[error("missing field/fields '{1}' for package {0}")]
    Missing(UrlOrPath, String),

    #[error("failed to convert location to URL for package {0}")]
    LocationToUrlConversionError(UrlOrPath, #[source] file_url::FileURLParseError),
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
                        ParseLockFileError::LocationToUrlConversionError(location, err)
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

impl TryFrom<CondaPackageData> for SourceRecord {
    type Error = ParseLockFileError;

    fn try_from(value: CondaPackageData) -> Result<Self, Self::Error> {
        let source = match value.location {
            UrlOrPath::Url(_) => unimplemented!(),
            UrlOrPath::Path(path) => PinnedPathSpec { path }.into(),
        };
        Ok(Self {
            package_record: value.package_record,
            source,
        })
    }
}

impl Matches<SourceRecord> for NamelessMatchSpec {
    fn matches(&self, pkg: &SourceRecord) -> bool {
        if !self.matches(&pkg.package_record) {
            return false;
        }

        if let Some(_) = &self.channel {
            // We don't have a channel in a source record. So if a matchspec requires that
            // information it can't match.
            return false;
        }

        true
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

impl Matches<SourceRecord> for MatchSpec {
    fn matches(&self, pkg: &SourceRecord) -> bool {
        if !self.matches(&pkg.package_record) {
            return false;
        }

        if let Some(_) = &self.channel {
            // We don't have a channel in a source record. So if a matchspec requires that
            // information it can't match.
            return false;
        }

        true
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

impl AsRef<PackageRecord> for SourceRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.package_record
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
