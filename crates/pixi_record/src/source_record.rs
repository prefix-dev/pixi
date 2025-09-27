use std::collections::{BTreeSet, HashMap};

use pixi_spec::SourceSpec;
use rattler_conda_types::{MatchSpec, Matches, NamelessMatchSpec, PackageRecord};
use rattler_digest::{Sha256, Sha256Hash};
use rattler_lock::{CondaPackageData, CondaSourceData};
use serde::{Deserialize, Serialize};

use crate::{ParseLockFileError, PinnedSourceSpec};

/// A record of a conda package that still requires building.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecord {
    /// Information about the conda package. This is metadata of the package
    /// after it has been build.
    pub package_record: PackageRecord,

    /// Exact definition of the source of the package.
    pub source: PinnedSourceSpec,

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

impl From<SourceRecord> for CondaPackageData {
    fn from(value: SourceRecord) -> Self {
        CondaPackageData::Source(CondaSourceData {
            package_record: value.package_record,
            location: value.source.into(),
            input: value.input_hash.map(|i| rattler_lock::InputHash {
                hash: i.hash,
                // TODO: fix this in rattler
                globs: Vec::from_iter(i.globs),
            }),
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            package_build_source: None,
        })
    }
}

impl TryFrom<CondaSourceData> for SourceRecord {
    type Error = ParseLockFileError;

    fn try_from(value: CondaSourceData) -> Result<Self, Self::Error> {
        Ok(Self {
            package_record: value.package_record,
            source: value.location.try_into()?,
            input_hash: value.input.map(|hash| InputHash {
                hash: hash.hash,
                globs: BTreeSet::from_iter(hash.globs),
            }),
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, SourceSpec::from(v)))
                .collect(),
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
