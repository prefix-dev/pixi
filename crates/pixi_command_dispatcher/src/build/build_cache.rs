use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    io::SeekFrom,
    path::PathBuf,
};

use crate::build::{MoveError, move_file, source_checkout_cache_key};
use async_fd_lock::{LockWrite, RwLockWriteGuard};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_record::PinnedSourceSpec;
use rattler_conda_types::{ChannelUrl, GenericVirtualPackage, Platform, RepoDataRecord};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use url::Url;
use xxhash_rust::xxh3::Xxh3;

/// A cache for caching build artifacts of a source checkout.
#[derive(Clone)]
pub struct BuildCache {
    root: PathBuf,
}

#[derive(Debug, Error)]
pub enum BuildCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] std::io::Error),

    /// Failed to move the build artifact
    #[error("failed to move build artifact from '{}' to cache '{}'", .0.display(), .1.display())]
    MoveError(PathBuf, PathBuf, #[source] MoveError),
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
        } = self;

        // Hash some of the keys
        let mut hasher = Xxh3::new();
        build.hash(&mut hasher);
        channel_urls.hash(&mut hasher);
        host_platform.hash(&mut hasher);
        host_virtual_packages.hash(&mut hasher);
        build_virtual_packages.hash(&mut hasher);
        let hash = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());

        format!("{name}-{version}-{subdir}-{hash}",)
    }
}

impl BuildCache {
    /// The version identifier that should be used for the cache directory.
    pub const CACHE_SUFFIX: &'static str = "v0";

    /// Constructs a new instance.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Returns a cache entry for the given source checkout and input from the
    /// cache. If the cache doesn't contain an entry for this source and input,
    /// it returns `None`.
    ///
    /// This function also returns a [`BuildCacheEntry`] which can be used to update
    /// the cache. The [`BuildCacheEntry`] also holds an exclusive lock on the cache
    /// which prevents other processes from accessing the cache entry. Drop
    /// the entry as soon as possible to release the lock.
    pub async fn entry(
        &self,
        source: &PinnedSourceSpec,
        input: &BuildInput,
    ) -> Result<(Option<CachedBuild>, BuildCacheEntry), BuildCacheError> {
        let input_key = input.hash_key();

        // Ensure the cache directory exists
        let cache_dir = self
            .root
            .join(source_checkout_cache_key(source))
            .join(input_key);
        fs_err::tokio::create_dir_all(&cache_dir)
            .await
            .map_err(|e| {
                BuildCacheError::IoError(
                    "creating cache directory".to_string(),
                    cache_dir.clone(),
                    e,
                )
            })?;

        // Try to acquire a lock on the cache file.
        let cache_file_path = cache_dir.join(".lock");
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
                    cache_file_path.clone(),
                    e,
                )
            })?;

        let mut locked_cache_file = cache_file.lock_write().await.map_err(|e| {
            BuildCacheError::IoError(
                "locking cache file".to_string(),
                cache_file_path.clone(),
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
                    cache_file_path.clone(),
                    e,
                )
            })?;

        let metadata = serde_json::from_str(&cache_file_contents).ok();
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

/// Cached result of calling `conda/getMetadata` on a build backend. This is
/// returned by [`SourceMetadataCache::entry`].
#[derive(Debug, Serialize, Deserialize)]
pub struct CachedBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CachedBuildSourceInfo>,
    pub record: RepoDataRecord,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedBuildSourceInfo {
    pub globs: BTreeSet<String>,
}

/// A cache entry returned by [`BuildCache::entry`] which enables
/// updating the cache.
///
/// As long as this entry is held, no other process can access this cache entry.
pub struct BuildCacheEntry {
    file: RwLockWriteGuard<tokio::fs::File>,
    cache_dir: PathBuf,
    cache_file_path: PathBuf,
}

impl BuildCacheEntry {
    /// Consumes this instance and writes the given metadata to the cache.
    pub async fn insert(
        mut self,
        mut metadata: CachedBuild,
    ) -> Result<RepoDataRecord, BuildCacheError> {
        // Move the file into the cache
        if let Ok(file_path) = metadata.record.url.to_file_path() {
            let file_name = file_path
                .file_name()
                .expect("the path cannot be empty because that wouldnt be a valid url");
            let destination = self.cache_dir.join(file_name);
            if let Err(err) = move_file(&file_path, &destination) {
                return Err(BuildCacheError::MoveError(file_path, destination, err));
            }

            metadata.record.url = Url::from_file_path(&destination)
                .expect("the cache directory path should be a valid url");
        }

        self.file.seek(SeekFrom::Start(0)).await.map_err(|e| {
            BuildCacheError::IoError(
                "seeking to start of cache file".to_string(),
                self.cache_file_path.clone(),
                e,
            )
        })?;
        let bytes = serde_json::to_vec(&metadata).expect("serialization to JSON should not fail");
        self.file.write_all(&bytes).await.map_err(|e| {
            BuildCacheError::IoError(
                "writing metadata to cache file".to_string(),
                self.cache_file_path.clone(),
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
                    self.cache_file_path.clone(),
                    e,
                )
            })?;

        Ok(metadata.record)
    }
}
