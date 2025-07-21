use std::fmt::Display;

use pixi_record::SourceRecord;
use rattler_conda_types::{PackageName, PackageRecord, VersionWithSource};
use serde::{Deserialize, Serialize};

/// A struct that uniquely identifies a single package in a channel.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct PackageIdentifier {
    pub name: PackageName,
    pub version: VersionWithSource,
    pub build: String,
    pub subdir: String,
}

impl Display for PackageIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}={}={}",
            &self.subdir,
            self.name.as_normalized(),
            self.version,
            self.build,
        )
    }
}

impl From<PackageRecord> for PackageIdentifier {
    fn from(value: PackageRecord) -> Self {
        Self {
            name: value.name,
            version: value.version,
            build: value.build,
            subdir: value.subdir,
        }
    }
}

impl<'a> From<&'a PackageRecord> for PackageIdentifier {
    fn from(record: &'a PackageRecord) -> Self {
        Self {
            name: record.name.clone(),
            version: record.version.clone(),
            build: record.build.clone(),
            subdir: record.subdir.clone(),
        }
    }
}

impl From<SourceRecord> for PackageIdentifier {
    fn from(record: SourceRecord) -> Self {
        record.package_record.into()
    }
}

impl<'a> From<&'a SourceRecord> for PackageIdentifier {
    fn from(record: &'a SourceRecord) -> Self {
        (&record.package_record).into()
    }
}
