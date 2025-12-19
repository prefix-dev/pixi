use std::path::{Path, PathBuf};

use async_fd_lock::{LockRead, LockWrite};
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

    /// Reads the cached metadata for the given key without holding the lock.
    /// Returns the metadata and its version number if it exists.
    ///
    /// The lock is released immediately after reading, so the metadata may
    /// become stale. Use `try_write` with version checking to detect if the
    /// cache was updated by another process.
    async fn read(&self, input: &Self::Key) -> Result<Option<(Self::Metadata, u64)>, Self::Error>
    where
        Self::Metadata: VersionedMetadata,
    {
        let cache_dir = self.root().join(input.hash_key());
        let cache_file_path = cache_dir.join(self.cache_file_name());

        // Try to open the cache file (may not exist yet)
        let cache_file = match tokio::fs::File::open(&cache_file_path).await {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Cache doesn't exist yet
                return Ok(None);
            }
            Err(e) => {
                return Err(Self::Error::from_io_error(
                    "opening cache file".to_string(),
                    cache_file_path,
                    e,
                ));
            }
        };

        let mut locked_cache_file = cache_file.lock_read().await.map_err(|e| {
            Self::Error::from_io_error(
                "locking cache file".to_string(),
                cache_file_path.clone(),
                e.error,
            )
        })?;

        // Read contents while holding lock
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

        // Release lock immediately
        drop(locked_cache_file);

        // Parse after lock is released
        let metadata: Self::Metadata = match serde_json::from_str(&cache_file_contents) {
            Ok(m) => m,
            Err(err) => {
                tracing::debug!(
                    "failed to parse cache file '{}': {}",
                    cache_file_path.display(),
                    err
                );
                // Invalid cache
                return Ok(None);
            }
        };

        let version = metadata.cache_version();
        Ok(Some((metadata, version)))
    }

    /// Tries to write metadata to the cache with optimistic locking.
    ///
    /// This method checks if the cache version matches the expected version
    /// before writing. If another process has updated the cache since it was
    /// read, this method returns `WriteResult::Conflict` with the newer metadata.
    ///
    /// The lock is held only during the version check and write operation.
    async fn try_write(
        &self,
        input: &Self::Key,
        metadata: Self::Metadata,
        expected_version: u64,
    ) -> Result<WriteResult<Self::Metadata>, Self::Error>
    where
        Self::Metadata: VersionedMetadata,
    {
        let cache_dir = self.root().join(input.hash_key());
        tokio::fs::create_dir_all(&cache_dir).await.map_err(|e| {
            Self::Error::from_io_error("creating cache directory".to_string(), cache_dir.clone(), e)
        })?;

        let cache_file_path = cache_dir.join(self.cache_file_name());

        // Open or create the cache file
        let cache_file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&cache_file_path)
            .await
            .map_err(|e| {
                Self::Error::from_io_error(
                    "opening cache file".to_string(),
                    cache_file_path.clone(),
                    e,
                )
            })?;

        // Acquire lock
        let mut locked_cache_file = cache_file.lock_write().await.map_err(|e| {
            Self::Error::from_io_error(
                "locking cache file".to_string(),
                cache_file_path.clone(),
                e.error,
            )
        })?;

        // Check if cache was updated by another process
        let mut current_contents = String::new();
        locked_cache_file
            .read_to_string(&mut current_contents)
            .await
            .map_err(|e| {
                Self::Error::from_io_error(
                    "reading cache file".to_string(),
                    cache_file_path.clone(),
                    e,
                )
            })?;

        // If cache exists and has different version, return conflict
        if !current_contents.is_empty()
            && let Ok(current_metadata) = serde_json::from_str::<Self::Metadata>(&current_contents)
            && current_metadata.cache_version() != expected_version
        {
            // Cache was updated by another process
            drop(locked_cache_file);
            return Ok(WriteResult::Conflict(current_metadata));
        }

        // Version matches (or cache is empty), write new data
        let mut new_metadata = metadata;
        new_metadata.set_cache_version(expected_version + 1);

        let bytes =
            serde_json::to_vec(&new_metadata).expect("serialization to JSON should not fail");

        // Write to file
        locked_cache_file.rewind().await.map_err(|e| {
            Self::Error::from_io_error(
                "seeking to start of cache file".to_string(),
                cache_file_path.clone(),
                e,
            )
        })?;

        locked_cache_file.write_all(&bytes).await.map_err(|e| {
            Self::Error::from_io_error(
                "writing metadata to cache file".to_string(),
                cache_file_path.clone(),
                e,
            )
        })?;

        // Truncate file to new size
        locked_cache_file
            .inner_mut()
            .set_len(bytes.len() as u64)
            .await
            .map_err(|e| {
                Self::Error::from_io_error(
                    "setting length of cache file".to_string(),
                    cache_file_path.clone(),
                    e,
                )
            })?;

        // Flush to ensure data is written
        locked_cache_file.flush().await.map_err(|e| {
            Self::Error::from_io_error("flushing cache file".to_string(), cache_file_path, e)
        })?;

        // Release lock
        drop(locked_cache_file);

        Ok(WriteResult::Written)
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

/// Trait for cached metadata that supports versioning for optimistic locking.
pub trait VersionedMetadata: CachedMetadata {
    /// Gets the current cache version
    fn cache_version(&self) -> u64;

    /// Sets the cache version
    fn set_cache_version(&mut self, version: u64);
}

/// Result of attempting to write to the cache with version checking.
#[derive(Debug)]
pub enum WriteResult<M> {
    /// The cache was successfully written.
    Written,
    /// The cache was updated by another process during computation.
    /// Contains the metadata that was written by the other process.
    Conflict(M),
}

/// Error trait to ensure consistent error handling across cache implementations.
pub trait CacheError: std::error::Error + Sized {
    /// Creates an error from an I/O error with context about the operation
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self;
}
