use std::{
    ffi::OsStr,
    hash::{DefaultHasher, Hash, Hasher},
    io::SeekFrom,
    path::PathBuf,
};

use async_fd_lock::{LockWrite, RwLockWriteGuard};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rattler_conda_types::{Platform, RepoDataRecord};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use url::Url;

use crate::build::SourceCheckout;

#[derive(Clone)]
pub struct BuildCache {
    root: PathBuf,
}

#[derive(Debug, Error)]
pub enum BuildCacheError {
    /// An I/O error occurred while reading or writing the cache.
    #[error("an IO error occurred while {0} {1}")]
    IoError(String, PathBuf, #[source] std::io::Error),
}

/// Defines additional input besides the source files that are used to compute
/// the metadata of a source checkout.
pub struct BuildInput {
    /// TODO: I think this should also include the build backend used! Maybe?

    /// The URL channels used in the build.
    pub channel_urls: Vec<Url>,

    /// The platform for which the metadata was computed.
    pub target_platform: Platform,

    /// The name of the package
    pub name: String,

    /// The version of the package to build
    pub version: String,

    /// The build string of the package to build
    pub build: String,
}

impl BuildInput {
    /// Computes a unique semi-human-readable hash for this key. Some parts of
    /// the input are hashes and others are included directly in the name this
    /// is to make it easier to identify the cache files.
    pub fn hash_key(&self) -> String {
        let BuildInput {
            channel_urls,
            target_platform,
            name,
            version,
            build,
        } = self;

        // Hash some of the keys
        let mut hasher = DefaultHasher::new();
        channel_urls.hash(&mut hasher);
        let hash = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());

        format!("{name}-{version}-{build}-{target_platform}-{hash}",)
    }
}

impl BuildCache {
    /// Constructs a new instance.
    ///
    /// An additional directory is created by this cache inside the passed root
    /// which includes a version number. This is to ensure that the cache is
    /// never corrupted if the format changes in the future.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: root.join("source-builds-v0"),
        }
    }

    /// Determine the path to the cache directory for a given request.
    fn source_cache_path(&self, source: &SourceCheckout) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        source.pinned.to_string().hash(&mut hasher);
        let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
        let path = match source.path.file_name().and_then(OsStr::to_str) {
            Some(name) => format!("{}-{}", name, unique_key),
            None => unique_key,
        };
        self.root.join(path)
    }

    pub async fn entry(
        &self,
        source: &SourceCheckout,
        input: &BuildInput,
    ) -> Result<(Option<CachedBuild>, CacheEntry), BuildCacheError> {
        let input_key = input.hash_key();

        // Ensure the cache directory exists
        let cache_dir = self.source_cache_path(source).join(input_key);
        tokio::fs::create_dir_all(&cache_dir).await.map_err(|e| {
            BuildCacheError::IoError("creating cache directory".to_string(), cache_dir.clone(), e)
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
            CacheEntry {
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
    pub source: Option<SourceInfo>,
    pub record: RepoDataRecord,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceInfo {
    pub globs: Vec<String>,
}

/// A cache entry returned by [`BuildCache::entry`] which enables
/// updating the cache.
///
/// As long as this entry is held, no other process can access this cache entry.
pub struct CacheEntry {
    file: RwLockWriteGuard<tokio::fs::File>,
    cache_dir: PathBuf,
    cache_file_path: PathBuf,
}

impl CacheEntry {
    /// Consumes this instance and writes the given metadata to the cache.
    pub async fn insert(mut self, metadata: CachedBuild) -> Result<(), BuildCacheError> {
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
        Ok(())
    }
}
