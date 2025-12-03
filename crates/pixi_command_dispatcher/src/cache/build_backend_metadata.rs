use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::{CondaPackageMetadata, procedures::conda_outputs::CondaOutput};
use pixi_record::{InputHash, PinnedSourceSpec, VariantValue};
use rattler_conda_types::ChannelUrl;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BuildEnvironment, PackageIdentifier, build::source_checkout_cache_key};

use super::common::{
    CacheError, CacheKey, CachedMetadata, MetadataCache, VersionedMetadata,
    WriteResult as CommonWriteResult,
};

// Re-export WriteResult with the correct type
pub type WriteResult = CommonWriteResult<CachedCondaMetadata>;

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
    root: PathBuf,
}

#[derive(Debug, Error)]
pub enum BuildBackendMetadataCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] std::io::Error),
}

/// Defines additional input besides the source files that are used to compute
/// the metadata of a source checkout. This is used to bucket the metadata.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct BuildBackendMetadataKey {
    /// The URLs of the channels that were used.
    pub channel_urls: Vec<ChannelUrl>,

    /// The build environment
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for source packages
    pub enabled_protocols: EnabledProtocols,

    /// The pinned source location
    pub pinned_source: PinnedSourceSpec,
}

impl BuildBackendMetadataCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl MetadataCache for BuildBackendMetadataCache {
    type Key = BuildBackendMetadataKey;
    type Metadata = CachedCondaMetadata;
    type Error = BuildBackendMetadataCacheError;

    fn root(&self) -> &Path {
        &self.root
    }

    fn cache_file_name(&self) -> &'static str {
        "metadata.json"
    }

    const CACHE_SUFFIX: &'static str = "v0";
}

impl CacheKey for BuildBackendMetadataKey {
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
            "{source_dir}/{}-{}",
            self.build_environment.host_platform,
            URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes())
        )
    }
}

impl CacheError for BuildBackendMetadataCacheError {
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self {
        BuildBackendMetadataCacheError::IoError(operation, path, error)
    }
}

/// Cached result of calling `conda/getMetadata` on a build backend. This is
/// returned by [`MetadataCache::read`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCondaMetadata {
    /// A randomly generated identifier that is generated for each metadata
    /// file.
    ///
    /// Cache information for each output is stored in a separate file, this ID
    /// is present in each file. This is to ensure that the cache can be
    /// invalidated if the metadata changes.
    pub id: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<InputHash>,

    /// Version number for optimistic locking. Incremented with each cache update.
    /// Used to detect when another process has updated the cache during computation.
    #[serde(default)]
    pub cache_version: u64,

    #[serde(flatten)]
    pub metadata: MetadataKind,

    /// The build variants that were used to generate this metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_variants: Option<BTreeMap<String, Vec<VariantValue>>>,
}

impl CachedMetadata for CachedCondaMetadata {}

impl VersionedMetadata for CachedCondaMetadata {
    fn cache_version(&self) -> u64 {
        self.cache_version
    }

    fn set_cache_version(&mut self, version: u64) {
        self.cache_version = version;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetadataKind {
    /// The result of calling `conda/getMetadata` on a build backend.
    GetMetadata { packages: Vec<CondaPackageMetadata> },

    /// The result of calling `conda/outputs` on a build backend.
    Outputs { outputs: Vec<CondaOutput> },
}

impl CachedCondaMetadata {
    /// Returns the unique package identifiers for the packages in this
    /// metadata.
    pub fn outputs(&self) -> Vec<PackageIdentifier> {
        match &self.metadata {
            MetadataKind::GetMetadata { packages } => packages
                .iter()
                .map(|pkg| PackageIdentifier {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    build: pkg.build.clone(),
                    subdir: pkg.subdir.to_string(),
                })
                .collect(),
            MetadataKind::Outputs { outputs } => outputs
                .iter()
                .map(|output| PackageIdentifier {
                    name: output.metadata.name.clone(),
                    version: output.metadata.version.clone(),
                    build: output.metadata.build.clone(),
                    subdir: output.metadata.subdir.to_string(),
                })
                .collect(),
        }
    }
}
