use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_build_discovery::EnabledProtocols;
use pixi_record::{InputHash, PinnedSourceSpec, SourceRecord};
use rattler_conda_types::{ChannelUrl, PackageName};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BuildEnvironment, build::source_checkout_cache_key};

use super::common::{CacheError, CacheKey, CachedMetadata, MetadataCache};

// Re-export CacheEntry with the correct generic type for this cache
pub type CacheEntry = super::common::CacheEntry<SourceMetadataCache>;

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
    root: PathBuf,
}

#[derive(Debug, Error)]
pub enum SourceMetadataCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] std::io::Error),
}

/// Defines additional input besides the source files that are used to compute
/// the metadata of a source checkout. This is used to bucket the metadata.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SourceMetadataKey {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// The URLs of the channels that were used.
    pub channel_urls: Vec<ChannelUrl>,

    /// The build environment
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,

    /// The pinned source location
    pub pinned_source: PinnedSourceSpec,
}

impl SourceMetadataCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl MetadataCache for SourceMetadataCache {
    type Key = SourceMetadataKey;
    type Metadata = CachedSourceMetadata;
    type Error = SourceMetadataCacheError;

    fn root(&self) -> &Path {
        &self.root
    }

    fn cache_file_name(&self) -> &'static str {
        "source_metadata.json"
    }

    const CACHE_SUFFIX: &'static str = "v0";
}

impl CacheKey for SourceMetadataKey {
    /// Computes a unique semi-human-readable hash for this key.
    fn hash_key(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.channel_urls.hash(&mut hasher);
        self.build_environment.build_platform.hash(&mut hasher);
        self.build_environment
            .build_virtual_packages
            .hash(&mut hasher);
        self.build_environment
            .host_virtual_packages
            .hash(&mut hasher);
        self.enabled_protocols.hash(&mut hasher);
        let source_dir = source_checkout_cache_key(&self.pinned_source);
        format!(
            "{source_dir}/{}/{}-{}",
            self.package.as_normalized(),
            self.build_environment.host_platform,
            URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes())
        )
    }
}

impl CacheError for SourceMetadataCacheError {
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self {
        SourceMetadataCacheError::IoError(operation, path, error)
    }
}

/// Cached result of calling `conda/getMetadata` on a build backend. This is
/// returned by [`SourceMetadataCache::entry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSourceMetadata {
    /// A randomly generated identifier that is generated for each metadata
    /// file.
    ///
    /// Cache information for each output is stored in a separate file, this ID
    /// is present in each file. This is to ensure that the cache can be
    /// invalidated if the metadata changes.
    pub id: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<InputHash>,

    /// The build variants that were used to generate this metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_variants: Option<BTreeMap<String, Vec<String>>>,

    #[serde(flatten)]
    pub metadata: Metadata,
}

impl CachedMetadata for CachedSourceMetadata {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct Metadata {
    /// All the source records for this particular package.
    pub records: Vec<SourceRecord>,
}
