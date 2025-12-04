use std::{
    io::SeekFrom,
    marker::PhantomData,
    path::{Path, PathBuf},
};

use async_fd_lock::{LockWrite, RwLockWriteGuard};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Core trait that defines the contract for a metadata cache.
///
/// This trait provides a default implementation for the `entry()` method that
/// handles the common caching logic, while allowing implementations to customize
/// the cache file name and error handling.
#[allow(async_fn_in_trait)]
pub trait MetadataCache: Clone + Sized {
    /// The version identifier for the cache directory
    const CACHE_SUFFIX: &'static str;

    /// The type of the cache key
    type Key: CacheKey;

    /// The type of the cached metadata
    type Metadata: CachedMetadata;

    /// The error type for cache operations
    type Error: CacheError;

    /// Returns the root directory for this cache
    fn root(&self) -> &Path;

    /// Returns the name of the cache file (e.g., "metadata.json")
    fn cache_file_name(&self) -> &'static str;

    /// Returns the cache entry for the given key.
    ///
    /// Returns the cached metadata if it exists and is still valid and a
    /// [`CacheEntry`] that can be used to update the cache. As long as the
    /// [`CacheEntry`] is held, another process cannot update the cache.
    async fn entry(
        &self,
        input: &Self::Key,
    ) -> Result<(Option<Self::Metadata>, CacheEntry<Self>), Self::Error> {
        // Locate the cache file and lock it.
        let cache_dir = self.root().join(input.hash_key());
        tokio::fs::create_dir_all(&cache_dir).await.map_err(|e| {
            Self::Error::from_io_error("creating cache directory".to_string(), cache_dir.clone(), e)
        })?;

        // Try to acquire a lock on the cache file.
        let cache_file_path = cache_dir.join(self.cache_file_name());
        let cache_file = tokio::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .truncate(false)
            .create(true)
            .open(&cache_file_path)
            .await
            .map_err(|e| {
                Self::Error::from_io_error(
                    "opening cache file".to_string(),
                    cache_file_path.clone(),
                    e,
                )
            })?;

        let mut locked_cache_file = cache_file.lock_write().await.map_err(|e| {
            Self::Error::from_io_error(
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
                Self::Error::from_io_error(
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
                path: cache_file_path,
                _phantom: PhantomData,
            },
        ))
    }
}

/// Trait for cache keys that can compute a unique hash
pub trait CacheKey {
    /// Computes a unique semi-human-readable hash for this key.
    fn hash_key(&self) -> String;
}

/// Trait for cached metadata types.
///
/// Implementors must be serializable and deserializable.
pub trait CachedMetadata: Serialize + DeserializeOwned {}

/// Error trait to ensure consistent error handling across cache implementations.
pub trait CacheError: std::error::Error + Sized {
    /// Creates an error from an I/O error with context about the operation
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self;
}

/// A cache entry returned by [`MetadataCache::entry`] which enables
/// updating the cache.
///
/// As long as this entry is held, no other process can access this cache entry.
#[derive(Debug)]
pub struct CacheEntry<C: MetadataCache> {
    file: RwLockWriteGuard<tokio::fs::File>,
    path: PathBuf,
    _phantom: PhantomData<C>,
}

impl<C: MetadataCache> CacheEntry<C> {
    /// Writes the given metadata to the cache.
    pub async fn write(&mut self, metadata: C::Metadata) -> Result<(), C::Error> {
        self.file.seek(SeekFrom::Start(0)).await.map_err(|e| {
            C::Error::from_io_error(
                "seeking to start of cache file".to_string(),
                self.path.clone(),
                e,
            )
        })?;

        let bytes = serde_json::to_vec(&metadata).expect("serialization to JSON should not fail");

        self.file.write_all(&bytes).await.map_err(|e| {
            C::Error::from_io_error(
                "writing metadata to cache file".to_string(),
                self.path.clone(),
                e,
            )
        })?;

        self.file
            .inner_mut()
            .set_len(bytes.len() as u64)
            .await
            .map_err(|e| {
                C::Error::from_io_error(
                    "setting length of cache file".to_string(),
                    self.path.clone(),
                    e,
                )
            })?;

        Ok(())
    }
}
