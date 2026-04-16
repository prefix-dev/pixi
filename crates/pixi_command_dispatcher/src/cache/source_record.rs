use super::common::{
    CacheError, CacheKeyString, CacheRevision, MetadataCache, MetadataCacheEntry, MetadataCacheKey,
    UpstreamCacheRef, VersionedCacheEntry, WriteResult as CommonWriteResult,
};
use crate::BuildEnvironment;
use crate::build::CanonicalSourceCodeLocation;
use crate::cache::build_backend_metadata::BuildBackendMetadataCache;
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
pub type WriteResult = CommonWriteResult<SourceRecordCacheEntry>;

/// A cache for caching the resolved metadata of a single source record
/// (a specific package name + variant combination).
///
/// Resolving a source record requires invoking the build backend and solving
/// build/host dependency environments, which can be time-consuming. This cache
/// stores the result for a given source record so that repeated requests with
/// the same inputs can be served without re-resolution.
#[derive(Clone, Debug)]
pub struct SourceRecordCache {
    root: AbsPathBuf,
}

#[derive(Debug, Clone, Error)]
pub enum SourceRecordCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] Arc<std::io::Error>),
}

/// Cache key for a single source record. Includes the variant so that
/// each name+variant combination maps to its own cache file.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SourceRecordCacheKey {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// The variants that identify this specific build output.
    pub variants: BTreeMap<String, VariantValue>,

    /// The URLs of the channels that were used.
    pub channel_urls: Vec<ChannelUrl>,

    /// The build environment
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,

    /// The pinned source location
    pub source: CanonicalSourceCodeLocation,

    /// The exclude-newer cutoff that was used when resolving. Different cutoffs
    /// can yield different dependency sets. `None` when there are no host or
    /// build dependencies.
    pub exclude_newer: Option<pixi_spec::ResolvedExcludeNewer>,
}

impl SourceRecordCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: AbsPathBuf) -> Self {
        Self { root }
    }
}

impl MetadataCache for SourceRecordCache {
    type Key = SourceRecordCacheKey;
    type Entry = SourceRecordCacheEntry;
    type Error = SourceRecordCacheError;

    fn root(&self) -> &Path {
        self.root.as_std_path()
    }

    const CACHE_SUFFIX: &'static str = "v0";
}

impl MetadataCacheKey<SourceRecordCache> for SourceRecordCacheKey {
    /// Computes a unique semi-human-readable string representation of the key.
    /// This is what is used as the cache file name.
    fn key(&self) -> CacheKeyString<SourceRecordCache> {
        let mut hasher = DefaultHasher::new();
        self.channel_urls.hash(&mut hasher);

        self.build_environment.build_platform.hash(&mut hasher);
        let mut build_virtual_packages = self.build_environment.build_virtual_packages.clone();
        build_virtual_packages.sort_by(|a, b| a.name.cmp(&b.name));
        build_virtual_packages.hash(&mut hasher);

        self.build_environment.host_platform.hash(&mut hasher);
        let mut host_virtual_packages = self.build_environment.host_virtual_packages.clone();
        host_virtual_packages.sort_by(|a, b| a.name.cmp(&b.name));
        host_virtual_packages.hash(&mut hasher);

        self.enabled_protocols.hash(&mut hasher);
        self.variants.hash(&mut hasher);
        self.exclude_newer.hash(&mut hasher);

        let source_dir = self.source.cache_unique_key();
        CacheKeyString::new(format!(
            "{source_dir}/{}-{}-{}",
            self.package.as_normalized(),
            self.build_environment
                .host_platform
                .to_string()
                .replace('-', "_"),
            URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes()),
        ))
    }
}

impl CacheError for SourceRecordCacheError {
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self {
        SourceRecordCacheError::IoError(operation, path, Arc::new(error))
    }
}

/// Cached result of resolving a single source record. This is returned by
/// [`MetadataCache::read`].
///
/// Contains all the data needed to reconstruct a `SourceRecord` except for
/// the manifest/build source locations, which are derived from the cache key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRecordCacheEntry {
    /// A revision identifier for this cache entry. Changes when the
    /// meaningful content of the entry changes.
    pub revision: CacheRevision<SourceRecordCache>,

    /// Version number for optimistic locking. Incremented with each cache update.
    /// Used to detect when another process has updated the cache during computation.
    #[serde(default)]
    pub cache_version: u64,

    /// Reference to the build backend metadata entry this was derived from.
    /// Contains the cache key (to locate the file) and the revision (to
    /// detect staleness).
    pub build_backend: UpstreamCacheRef<BuildBackendMetadataCache>,

    /// The cached source record data.
    #[serde(flatten)]
    pub record: CachedSourceRecord,
}

/// A cached version of a `SourceRecord` but with a few elements removed
/// because they can be derived from the input instead (like the manifest source).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedSourceRecord {
    pub package_record: PackageRecord,

    /// The variants that uniquely identify the way this package was built.
    pub variants: BTreeMap<String, VariantValue>,

    /// Specifies which packages are expected to be installed as source packages
    /// and from which location.
    pub sources: HashMap<String, SourceLocationSpec>,

    /// The timestamps of the newest packages in the build/host environments,
    /// or `None` when there are no host or build dependencies.
    pub timestamp: Option<pixi_spec::SourceTimestamps>,
}

impl MetadataCacheEntry<SourceRecordCache> for SourceRecordCacheEntry {
    fn revision(&self) -> &CacheRevision<SourceRecordCache> {
        &self.revision
    }
}

impl VersionedCacheEntry<SourceRecordCache> for SourceRecordCacheEntry {
    fn cache_version(&self) -> u64 {
        self.cache_version
    }

    fn set_cache_version(&mut self, version: u64) {
        self.cache_version = version;
    }
}
