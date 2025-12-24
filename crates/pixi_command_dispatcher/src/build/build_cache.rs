use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    hash::{Hash, Hasher},
    io::SeekFrom,
    path::PathBuf,
};

use crate::build::{SourceCodeLocation, source_checkout_cache_key};
use async_fd_lock::{LockWrite, RwLockWriteGuard};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ordermap::OrderMap;
use pixi_build_discovery::{BackendInitializationParams, DiscoveredBackend};
use pixi_build_types::{ProjectModelV1, TargetSelectorV1};
use pixi_path::{AbsPathBuf, AbsPresumedDirPath, AbsPresumedDirPathBuf, AbsPresumedFilePathBuf};
use pixi_record::{PinnedSourceSpec, VariantValue};
use pixi_stable_hash::{StableHashBuilder, json::StableJson, map::StableMap};
use rattler_conda_types::{ChannelUrl, GenericVirtualPackage, Platform, RepoDataRecord};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use xxhash_rust::xxh3::Xxh3;

/// A cache for caching build artifacts of a source checkout.
#[derive(Clone)]
pub struct BuildCache {
    pub(crate) root: AbsPresumedDirPathBuf,
}

#[derive(Debug, Error)]
pub enum BuildCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, AbsPathBuf, #[source] std::io::Error),
}

/// Defines additional input besides the source files that are used to compute
/// the metadata of a source checkout.
pub struct BuildInput {
    /// The URL channels used in the build.
    pub channel_urls: Vec<ChannelUrl>,

    /// The name of the package
    pub name: String,

    /// The version of the package to build
    pub version: String,

    /// The build string of the package to build
    pub build: String,

    /// The platform for which the metadata was computed.
    pub subdir: String,

    /// The host platform
    pub host_platform: Platform,

    /// The virtual packages of the target host
    pub host_virtual_packages: Vec<GenericVirtualPackage>,

    /// The virtual packages used to build the package
    pub build_virtual_packages: Vec<GenericVirtualPackage>,

    /// The specific variant values for this build. Different variants result
    /// in different cache keys to ensure they are cached separately.
    pub variants: Option<BTreeMap<String, VariantValue>>,
}

impl BuildInput {
    /// Computes a unique semi-human-readable hash for this key. Some parts of
    /// the input are hashes and others are included directly in the name this
    /// is to make it easier to identify the cache files.
    pub fn hash_key(&self) -> String {
        let BuildInput {
            channel_urls,
            name,
            version,
            build,
            subdir,
            host_platform,
            host_virtual_packages,
            build_virtual_packages,
            variants,
        } = self;

        // Hash some of the keys
        let mut hasher = Xxh3::new();
        build.hash(&mut hasher);
        channel_urls.hash(&mut hasher);
        host_platform.hash(&mut hasher);

        let mut sorted_host_virtual_packages = host_virtual_packages.clone();
        sorted_host_virtual_packages.sort_by(|a, b| a.name.cmp(&b.name));
        sorted_host_virtual_packages.hash(&mut hasher);

        let mut sorted_build_virtual_packages = build_virtual_packages.clone();
        sorted_build_virtual_packages.sort_by(|a, b| a.name.cmp(&b.name));
        sorted_build_virtual_packages.hash(&mut hasher);

        // Include variants in the hash to ensure different variant values
        // get different cache keys. BTreeMap is already sorted by key, so we
        // can hash it directly for deterministic results.
        variants.hash(&mut hasher);

        let hash = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());

        format!("{name}-{version}-{subdir}-{hash}",)
    }
}

impl BuildCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: AbsPresumedDirPathBuf) -> Self {
        Self { root }
    }

    /// Returns a cache entry for the given source checkout and input from the
    /// cache. If the cache doesn't contain an entry for this source and input,
    /// it returns `None`.
    ///
    /// This function also returns a [`BuildCacheEntry`] which can be used to
    /// update the cache. The [`BuildCacheEntry`] also holds an exclusive
    /// lock on the cache which prevents other processes from accessing the
    /// cache entry. Drop the entry as soon as possible to release the lock.
    pub async fn entry(
        &self,
        source: &PinnedSourceSpec,
        input: &BuildInput,
    ) -> Result<(Option<CachedBuild>, BuildCacheEntry), BuildCacheError> {
        let input_key = input.hash_key();
        tracing::debug!(
            source = %source,
            input_key = %input_key,
            name = %input.name,
            version = %input.version,
            subdir = %input.subdir,
            host_platform = %input.host_platform,
            build = %input.build,
            channel_urls = ?input.channel_urls,
            host_virtual_packages = ?input.host_virtual_packages,
            build_virtual_packages = ?input.build_virtual_packages,
            variants = ?input.variants,
            "opening source build cache entry",
        );

        // Ensure the cache directory exists
        let cache_dir = self
            .root
            .join(source_checkout_cache_key(source))
            .join(&input_key);
        let cache_dir = cache_dir
            .create_dir_all()
            .map_err(|e| {
                BuildCacheError::IoError(
                    "creating cache directory".to_string(),
                    cache_dir.clone(),
                    e,
                )
            })?
            .to_path_buf();

        // Try to acquire a lock on the cache file.
        let cache_file_path = cache_dir.join(".lock").into_assume_file();
        let cache_file = tokio::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .truncate(false)
            .create(true)
            .open(&cache_file_path)
            .await
            .map_err(|e| {
                BuildCacheError::IoError(
                    "opening cache file".to_string(),
                    cache_file_path.clone().into(),
                    e,
                )
            })?;

        let mut locked_cache_file = cache_file.lock_write().await.map_err(|e| {
            BuildCacheError::IoError(
                "locking cache file".to_string(),
                cache_file_path.clone().into(),
                e.error,
            )
        })?;

        // Try to parse the contents of the file
        let mut cache_file_contents = String::new();
        locked_cache_file
            .read_to_string(&mut cache_file_contents)
            .await
            .map_err(|e| {
                BuildCacheError::IoError(
                    "reading cache file".to_string(),
                    cache_file_path.clone().into(),
                    e,
                )
            })?;

        let metadata: Option<CachedBuild> = serde_json::from_str(&cache_file_contents).ok();
        if let Some(existing) = metadata.as_ref() {
            tracing::debug!(
                source = %source,
                input_key = %input_key,
                package = ?existing.record.package_record.name,
                build = %existing.record.package_record.build,
                "found cached build metadata",
            );
        } else {
            tracing::debug!(
                source = %source,
                input_key = %input_key,
                "no cached build metadata found",
            );
        }
        Ok((
            metadata,
            BuildCacheEntry {
                file: locked_cache_file,
                cache_dir,
                cache_file_path,
            },
        ))
    }
}

/// Cached result of calling `conda/getMetadata` on a build backend.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CachedBuildSourceInfo>,
    pub record: RepoDataRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedBuildSourceInfo {
    /// Glob patterns that define which files affect the build. If any matching
    /// file changes, the build should be considered stale.
    #[serde(default, skip_serializing_if = "BinaryHeap::is_empty")]
    pub input_globs: BinaryHeap<String>,

    /// The actual files that matched the globs at the time of the build. This
    /// allows detecting file deletions and additions by comparing against
    /// current glob matches.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub input_files: BTreeSet<PathBuf>,

    /// The packages that were used during the build process.
    #[serde(default)]
    pub build: BuildHostEnvironment,
    /// The packages that were installed in the host environment.
    #[serde(default)]
    pub host: BuildHostEnvironment,

    /// A hash of the package build input. If this changes, the build should be
    /// considered stale.
    #[serde(default)]
    pub package_build_input_hash: Option<PackageBuildInputHash>,
}

#[serde_as]
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct BuildHostEnvironment {
    /// Describes the packages that were installed in the host environment.
    pub packages: Vec<BuildHostPackage>,
}

#[serde_as]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildHostPackage {
    /// The repodata record of the package.
    #[serde(flatten)]
    pub repodata_record: RepoDataRecord,

    /// The source location from which the package was built.
    pub source: Option<SourceCodeLocation>,
}

/// A cache entry returned by [`BuildCache::entry`] which enables
/// updating the cache.
///
/// As long as this entry is held, no other process can access this cache entry.
#[derive(Debug)]
pub struct BuildCacheEntry {
    file: RwLockWriteGuard<tokio::fs::File>,
    cache_dir: AbsPresumedDirPathBuf,
    cache_file_path: AbsPresumedFilePathBuf,
}

impl BuildCacheEntry {
    /// The directory where the cache is stored.
    pub fn cache_dir(&self) -> &AbsPresumedDirPath {
        &self.cache_dir
    }

    /// Write the given metadata to the cache file.
    pub async fn insert(
        &mut self,
        metadata: CachedBuild,
    ) -> Result<RepoDataRecord, BuildCacheError> {
        self.file.seek(SeekFrom::Start(0)).await.map_err(|e| {
            BuildCacheError::IoError(
                "seeking to start of cache file".to_string(),
                self.cache_file_path.clone().into(),
                e,
            )
        })?;
        let bytes = serde_json::to_vec(&metadata).expect("serialization to JSON should not fail");
        self.file.write_all(&bytes).await.map_err(|e| {
            BuildCacheError::IoError(
                "writing metadata to cache file".to_string(),
                self.cache_file_path.clone().into(),
                e,
            )
        })?;
        self.file
            .inner_mut()
            .set_len(bytes.len() as u64)
            .await
            .map_err(|e| {
                BuildCacheError::IoError(
                    "setting length of cache file".to_string(),
                    self.cache_file_path.clone().into(),
                    e,
                )
            })?;

        tracing::debug!(
            cache_file = %self.cache_file_path.display(),
            package = ?metadata.record.package_record.name,
            build = %metadata.record.package_record.build,
            "updated source build cache entry",
        );

        Ok(metadata.record)
    }
}

/// A builder for creating a stable hash of the package build input.
///
/// This is used to compute a singular hash that changes when a rebuild is
/// warranted.
pub struct PackageBuildInputHashBuilder<'a> {
    /// The project model itself. Contains dependencies and more.
    pub project_model: Option<&'a ProjectModelV1>,

    /// The backend specific configuration
    pub configuration: Option<&'a serde_json::Value>,

    /// Target specific backend configuration
    pub target_configuration: Option<&'a OrderMap<TargetSelectorV1, serde_json::Value>>,
}

impl PackageBuildInputHashBuilder<'_> {
    pub fn finish(self) -> PackageBuildInputHash {
        let mut hasher = Xxh3::new();
        StableHashBuilder::new()
            .field("project_model", &self.project_model)
            .field("configuration", &self.configuration.map(StableJson::new))
            .field(
                "target_configuration",
                &self.target_configuration.map(|config| {
                    StableMap::new(config.iter().map(|(k, v)| (k, StableJson::new(v))))
                }),
            )
            .finish(&mut hasher);
        PackageBuildInputHash(hasher.finish())
    }
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Hash)]
#[repr(transparent)]
pub struct PackageBuildInputHash(u64);

impl<'a> From<&'a DiscoveredBackend> for PackageBuildInputHash {
    fn from(value: &'a DiscoveredBackend) -> Self {
        let BackendInitializationParams {
            project_model,
            configuration,
            target_configuration,

            // These fields are not relevant for the package build input hash
            workspace_root: _,
            build_source: _,
            source_anchor: _,
            manifest_path: _,
        } = &value.init_params;

        PackageBuildInputHashBuilder {
            project_model: project_model.as_ref(),
            configuration: configuration.as_ref(),
            target_configuration: target_configuration.as_ref(),
        }
        .finish()
    }
}
