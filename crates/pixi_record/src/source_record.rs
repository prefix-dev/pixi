use std::collections::{BTreeMap, BTreeSet, HashMap};

use pixi_spec::SourceSpec;
use rattler_conda_types::{
    MatchSpec, Matches, NoArchType, PackageName, PackageRecord, PackageUrl, VersionWithSource,
    package::RunExportsJson,
};
use rattler_digest::{Md5Hash, Sha256, Sha256Hash};
use rattler_lock::{CondaPackageData, CondaSourceData};
use serde::{Deserialize, Serialize};

use crate::{ParseLockFileError, PinnedSourceSpec, SelectedVariant};

/// A record of a conda package that still requires building.
/// This is stored in the lock file and doesn't include version/build information.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecord {
    /// The name of the package
    pub name: PackageName,

    /// Exact definition of the source of the package.
    pub source: PinnedSourceSpec,

    /// Conda-build variants used to disambiguate between multiple source packages
    /// at the same location.
    pub variants: SelectedVariant,

    /// Specification of packages this package depends on
    pub depends: Vec<String>,

    /// Additional constraints on packages
    pub constrains: Vec<String>,

    /// Experimental: additional dependencies grouped by feature name
    pub experimental_extra_depends: BTreeMap<String, Vec<String>>,

    /// The specific license of the package
    pub license: Option<String>,

    /// Package identifiers of packages that are equivalent to this package but
    /// from other ecosystems (e.g., PyPI)
    pub purls: Option<BTreeSet<PackageUrl>>,

    /// The hash of the input that was used to build the metadata of the
    /// package. This can be used to verify that the metadata is still valid.
    ///
    /// If this is `None`, the input hash was not computed or is not relevant
    /// for this record. The record can always be considered up to date.
    pub input_hash: Option<InputHash>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceSpec>,

    /// Python site-packages path if this is a Python package
    pub python_site_packages_path: Option<String>,
}

/// A source record with full metadata contains the complete package information after it has been
/// resolved/built, including version and build information.
/// This is used during solving and building, but not stored in the lock file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecordWithMetadata {
    /// The base source record
    pub source_record: SourceRecord,

    /// The version of the package
    pub version: VersionWithSource,

    /// The build string of the package
    pub build: String,

    /// The build number of the package
    pub build_number: rattler_conda_types::BuildNumber,

    /// The subdir of the package
    pub subdir: String,

    /// Optionally the architecture the package supports
    pub arch: Option<String>,

    /// Optionally the platform the package supports
    pub platform: Option<String>,

    /// MD5 hash of the package archive
    pub md5: Option<Md5Hash>,

    /// SHA256 hash of the package archive
    pub sha256: Option<Sha256Hash>,

    /// Size of the package archive in bytes
    pub size: Option<u64>,

    /// Track features
    pub track_features: Vec<String>,

    /// Features (deprecated)
    pub features: Option<String>,

    /// License family
    pub license_family: Option<String>,

    /// Timestamp of when the package was built
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Run exports information
    pub run_exports: Option<RunExportsJson>,

    /// NoArch type
    pub noarch: NoArchType,

    /// Legacy bz2 MD5 hash
    pub legacy_bz2_md5: Option<Md5Hash>,

    /// Legacy bz2 size
    pub legacy_bz2_size: Option<u64>,
}

impl From<SourceRecordWithMetadata> for PackageRecord {
    fn from(value: SourceRecordWithMetadata) -> Self {
        PackageRecord {
            name: value.source_record.name,
            version: value.version,
            build: value.build,
            build_number: value.build_number,
            subdir: value.subdir,
            depends: value.source_record.depends,
            constrains: value.source_record.constrains,
            arch: value.arch,
            platform: value.platform,
            md5: value.md5,
            sha256: value.sha256,
            size: value.size,
            license: value.source_record.license,
            license_family: value.license_family,
            purls: value.source_record.purls,
            track_features: value.track_features,
            features: value.features,
            timestamp: value.timestamp,
            run_exports: value.run_exports,
            experimental_extra_depends: value.source_record.experimental_extra_depends,
            noarch: value.noarch,
            legacy_bz2_md5: value.legacy_bz2_md5,
            legacy_bz2_size: value.legacy_bz2_size,
            python_site_packages_path: value.source_record.python_site_packages_path,
        }
    }
}

impl SourceRecordWithMetadata {
    /// Convert to a PackageRecord
    pub fn as_package_record(&self) -> PackageRecord {
        self.clone().into()
    }
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
            name: value.name,
            location: value.source.into(),
            variants: value.variants,
            depends: value.depends,
            constrains: value.constrains,
            experimental_extra_depends: value.experimental_extra_depends,
            license: value.license,
            purls: value.purls,
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            input: value.input_hash.map(|i| rattler_lock::InputHash {
                hash: i.hash,
                globs: Vec::from_iter(i.globs),
            }),
            package_build_source: None,
            python_site_packages_path: value.python_site_packages_path,
        })
    }
}

impl TryFrom<CondaSourceData> for SourceRecord {
    type Error = ParseLockFileError;

    fn try_from(value: CondaSourceData) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            source: value.location.try_into()?,
            variants: value.variants,
            depends: value.depends,
            constrains: value.constrains,
            experimental_extra_depends: value.experimental_extra_depends,
            license: value.license,
            purls: value.purls,
            input_hash: value.input.map(|hash| InputHash {
                hash: hash.hash,
                globs: BTreeSet::from_iter(hash.globs),
            }),
            sources: value
                .sources
                .into_iter()
                .map(|(k, v)| (k, SourceSpec::from(v)))
                .collect(),
            python_site_packages_path: value.python_site_packages_path,
        })
    }
}

impl Matches<SourceRecord> for MatchSpec {
    fn matches(&self, pkg: &SourceRecord) -> bool {
        // Check if the name matches
        if let Some(ref name) = self.name {
            if name != &pkg.name {
                return false;
            }
        }

        if self.channel.is_some() {
            // We don't have a channel in a source record. So if a matchspec requires that
            // information it can't match.
            return false;
        }

        // For source packages, version, build, and build_number are not stored
        // in the lock file, so we only match by name
        true
    }
}
