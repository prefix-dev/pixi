use super::common::{
    CacheError, CacheKeyString, CacheRevision, MetadataCache, MetadataCacheEntry, MetadataCacheKey,
    VersionedCacheEntry, WriteResult as CommonWriteResult,
};
use crate::build::CanonicalSourceCodeLocation;
use crate::input_hash::{ConfigurationHash, ProjectModelHash};
use rattler_conda_types::PackageName;

use crate::BuildEnvironment;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::procedures::conda_outputs::CondaOutput;
use pixi_path::AbsPathBuf;
use pixi_record::VariantValue;
use rattler_conda_types::ChannelUrl;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, BinaryHeap};
use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;

// Re-export WriteResult with the correct type
pub type WriteResult = CommonWriteResult<BuildBackendMetadataCacheEntry>;

/// A cache for caching the metadata of a source checkout.
///
/// To request metadata for a source checkout we need to invoke the build
/// backend associated with the given source checkout. This operation can be
/// time-consuming so we want to avoid having to query the build backend.
///
/// This cache stores the raw response for a given source checkout together with
/// some additional properties to determine if the cache is still valid.
#[derive(Clone, Debug)]
pub struct BuildBackendMetadataCache {
    root: AbsPathBuf,
}

#[derive(Debug, Clone, Error)]
pub enum BuildBackendMetadataCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] Arc<std::io::Error>),
}

/// Defines additional input besides the source files that are used to compute
/// the metadata of a source checkout. This is used to bucket the metadata.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct BuildBackendMetadataCacheKey {
    /// The URLs of the channels that were used.
    pub channel_urls: Vec<ChannelUrl>,

    /// The build environment
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,

    /// The pinned source location
    pub source: CanonicalSourceCodeLocation,
}

impl BuildBackendMetadataCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: AbsPathBuf) -> Self {
        Self { root }
    }
}

impl MetadataCache for BuildBackendMetadataCache {
    type Key = BuildBackendMetadataCacheKey;
    type Entry = BuildBackendMetadataCacheEntry;
    type Error = BuildBackendMetadataCacheError;

    fn root(&self) -> &Path {
        self.root.as_std_path()
    }

    const CACHE_SUFFIX: &'static str = "v0";
}

impl MetadataCacheKey<BuildBackendMetadataCache> for BuildBackendMetadataCacheKey {
    /// Computes a unique semi-human-readable string representation of the key.
    /// This is what is used as the cache file name.
    fn key(&self) -> CacheKeyString<BuildBackendMetadataCache> {
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
        CacheKeyString::new(format!(
            "{source_dir}/{}-{}",
            self.build_environment
                .host_platform
                .to_string()
                .replace('-', "_"),
            URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes())
        ))
    }
}

impl CacheError for BuildBackendMetadataCacheError {
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self {
        BuildBackendMetadataCacheError::IoError(operation, path, Arc::new(error))
    }
}

/// Cached result of calling `conda/outputs` on a build backend. This is
/// returned by [`MetadataCache::read`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildBackendMetadataCacheEntry {
    /// A revision identifier for this cache entry. Changes when the
    /// meaningful content of the entry changes. Downstream caches store
    /// this to detect when their data is stale.
    pub revision: CacheRevision<BuildBackendMetadataCache>,

    /// Version number for optimistic locking. Incremented with each cache
    /// update. Used to detect when another process has updated the cache
    /// during computation.
    #[serde(default)]
    pub cache_version: u64,

    /// The hash of the project model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_model_hash: Option<ProjectModelHash>,

    /// The hash of the build configuration (from `[package.build.config]`).
    /// This ensures that changes to the build configuration invalidate the
    /// cache even if the project model hasn't changed.
    #[serde(default)]
    pub configuration_hash: ConfigurationHash,

    /// The pinned location of the source code. Although the specification of
    /// where to find the source is part of the `project_model_hash`, the
    /// resolved location is not.
    ///
    /// This is also part of the cache shard but for debug purposes we store
    /// it in this file as well.
    pub source: CanonicalSourceCodeLocation,

    /// The build variants that were used to generate this metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub build_variants: BTreeMap<String, Vec<VariantValue>>,

    /// The build variant files
    #[serde(default, skip_serializing_if = "BinaryHeap::is_empty")]
    pub build_variant_files: BinaryHeap<PathBuf>,

    /// Globs of files from which the metadata was derived. Globs require
    /// recursively iterating the filesystem which can be particularly slow so
    /// we prefer to store direct file paths instead. However, this does not
    /// work for all backends so we also support globs.
    ///
    /// If the source itself is immutable this is None.
    #[serde(default, skip_serializing_if = "BinaryHeap::is_empty")]
    pub input_globs: BinaryHeap<String>,

    /// Paths relative to the source checkout of files that were used to
    /// determine the metadata. This is the result of the matching the globs
    /// against the filesystem.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub input_files: BTreeSet<PathBuf>,

    /// The timestamp of when the metadata was computed.
    pub timestamp: std::time::SystemTime,

    /// The outputs as reported by the build backend.
    pub outputs: Vec<CondaOutput>,
}

impl MetadataCacheEntry<BuildBackendMetadataCache> for BuildBackendMetadataCacheEntry {
    fn revision(&self) -> &CacheRevision<BuildBackendMetadataCache> {
        &self.revision
    }
}

impl VersionedCacheEntry<BuildBackendMetadataCache> for BuildBackendMetadataCacheEntry {
    fn cache_version(&self) -> u64 {
        self.cache_version
    }

    fn set_cache_version(&mut self, version: u64) {
        self.cache_version = version;
    }
}

impl BuildBackendMetadataCacheEntry {
    /// Returns the unique package identifiers for the packages in this
    /// metadata.
    pub fn output_names(&self) -> Vec<PackageName> {
        self.outputs
            .iter()
            .map(|output| output.metadata.name.clone())
            .collect()
    }
}
