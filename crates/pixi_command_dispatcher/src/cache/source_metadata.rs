use super::common::{
    CacheError, CacheKey, CachedMetadata, MetadataCache, WriteResult as CommonWriteResult,
};
use crate::build::CanonicalSourceCodeLocation;
use crate::cache::build_backend_metadata::CachedCondaMetadataId;
use crate::{BuildEnvironment, cache::common::VersionedMetadata};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_build_discovery::EnabledProtocols;
use pixi_path::AbsPathBuf;
use pixi_spec::SourceLocationSpec;
use pixi_variant::VariantValue;
use rattler_conda_types::{ChannelUrl, PackageName, PackageRecord};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;

// Re-export WriteResult with the correct type
pub type WriteResult = CommonWriteResult<CachedSourceMetadata>;

/// A cache for caching the metadata of a source checkout.
///
/// To request metadata for a source checkout we need to invoke the build
/// backend associated with the given source checkout. This operation can be
/// time-consuming so we want to avoid having to query the build backend.
///
/// This cache stores the raw response for a given source checkout together with
/// some additional properties to determine if the cache is still valid.
#[derive(Clone, Debug)]
pub struct SourceMetadataCache {
    root: AbsPathBuf,
}

#[derive(Debug, Clone, Error)]
pub enum SourceMetadataCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] Arc<std::io::Error>),
}

/// Defines additional input besides the source files that are used to compute
/// the metadata of a source checkout. This is used to bucket the metadata.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SourceMetadataCacheShard {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// The URLs of the channels that were used.
    pub channel_urls: Vec<ChannelUrl>,

    /// The build environment
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,

    /// The pinned source location
    pub source: CanonicalSourceCodeLocation,
}

impl SourceMetadataCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: AbsPathBuf) -> Self {
        Self { root }
    }
}

impl MetadataCache for SourceMetadataCache {
    type Key = SourceMetadataCacheShard;
    type Metadata = CachedSourceMetadata;
    type Error = SourceMetadataCacheError;

    fn root(&self) -> &Path {
        self.root.as_std_path()
    }

    const CACHE_SUFFIX: &'static str = "v0";
}

impl CacheKey for SourceMetadataCacheShard {
    /// Computes a unique semi-human-readable hash for this key.
    fn hash_key(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.channel_urls.hash(&mut hasher);
        self.build_environment.build_platform.hash(&mut hasher);

        let mut build_virtual_packages = self.build_environment.build_virtual_packages.clone();
        build_virtual_packages.sort_by(|a, b| a.name.cmp(&b.name));
        build_virtual_packages.hash(&mut hasher);

        let mut host_virtual_packages = self.build_environment.host_virtual_packages.clone();
        host_virtual_packages.sort_by(|a, b| a.name.cmp(&b.name));
        host_virtual_packages.hash(&mut hasher);

        self.enabled_protocols.hash(&mut hasher);

        let source_dir = self.source.cache_unique_key();
        format!(
            "{source_dir}/{}-{}-{}",
            self.package.as_normalized(),
            self.build_environment.host_platform,
            URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes())
        )
    }
}

impl CacheError for SourceMetadataCacheError {
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self {
        SourceMetadataCacheError::IoError(operation, path, Arc::new(error))
    }
}

/// Cached result of calling `conda/getMetadata` on a build backend. This is
/// returned by [`MetadataCache::read`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSourceMetadata {
    /// A randomly generated identifier that is generated for each metadata
    /// file.
    pub id: CachedSourceMetadataId,

    /// Version number for optimistic locking. Incremented with each cache update.
    /// Used to detect when another process has updated the cache during computation.
    #[serde(default)]
    pub cache_version: u64,

    /// The id of the backend metadata that was used to compute this metadata.
    pub cached_conda_metadata_id: CachedCondaMetadataId,

    /// The source records
    pub records: Vec<CachedSourceRecord>,
}

/// A cached version of a `SourceRecord` but with a few elements removed
/// because they can be derived from the input instead like te manifest source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSourceRecord {
    pub package_record: PackageRecord,

    /// The variants that uniquely identify the way this package was built.
    pub variants: BTreeMap<String, VariantValue>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceLocationSpec>,
}

impl CachedMetadata for CachedSourceMetadata {}

impl VersionedMetadata for CachedSourceMetadata {
    fn cache_version(&self) -> u64 {
        self.cache_version
    }

    fn set_cache_version(&mut self, version: u64) {
        self.cache_version = version;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq)]
#[serde(transparent)]
pub struct CachedSourceMetadataId(u64);

impl CachedSourceMetadataId {
    pub fn random() -> Self {
        Self(rand::random())
    }
}
