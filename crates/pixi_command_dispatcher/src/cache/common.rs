//! Trait-based framework for caching serializable metadata to JSON files
//! on disk. Implementors plug in a key, an entry (carrying a
//! [`CacheRevision`] that downstream caches store to detect staleness),
//! and an error type; the trait provides file I/O, locking, and
//! serialization. [`VersionedCacheEntry`] adds a monotonic version counter
//! for cross-process optimistic locking.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use async_fd_lock::{LockRead, LockWrite};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Core trait that defines the contract for a metadata cache.
///
/// Implementations specify the key, entry, and error types along with the
/// cache root directory. The default [`Self::read`] and [`Self::try_write`]
/// methods handle file I/O, locking, and JSON (de)serialization so that
/// implementations only need to provide the associated types and [`Self::root`].
#[allow(async_fn_in_trait)]
pub trait MetadataCache: Clone + Sized {
    /// A version suffix appended to the cache directory path. Bump this when
    /// the on-disk format changes in a backwards-incompatible way so that old
    /// cache files are not read by new code.
    const CACHE_SUFFIX: &'static str;

    /// The key type used to look up entries. Each key maps to a single JSON
    /// file on disk.
    type Key: MetadataCacheKey<Self>;

    /// The entry type stored in each cache file. Must be serializable and
    /// must carry a [`CacheRevision`] so downstream caches can track
    /// staleness.
    type Entry: MetadataCacheEntry<Self>;

    /// The error type returned by cache operations.
    type Error: CacheError;

    /// Returns the root directory where cache files are stored.
    fn root(&self) -> &Path;

    /// Reads the cached metadata for the given key without holding the lock.
    /// Returns the metadata and its version number if it exists.
    ///
    /// The lock is released immediately after reading, so the metadata may
    /// become stale. Use `try_write` with version checking to detect if the
    /// cache was updated by another process.
    async fn read(&self, input: &Self::Key) -> Result<Option<Self::Entry>, Self::Error>
    where
        Self::Entry: VersionedCacheEntry<Self>,
    {
        let cache_file_path = self.cache_file_path(input);

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
        let metadata: Self::Entry = match serde_json::from_str(&cache_file_contents) {
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

        Ok(Some(metadata))
    }

    /// Tries to write metadata to the cache with optimistic locking.
    ///
    /// This method checks if the cache version matches the expected version
    /// before writing. If another process has updated the cache since it was
    /// read, this method returns `WriteResult::Conflict` with the newer metadata.
    /// `metadata` must already carry `expected_version + 1`.
    ///
    /// The lock is held only during the version check and write operation.
    async fn try_write(
        &self,
        input: &Self::Key,
        metadata: &Self::Entry,
        expected_version: u64,
    ) -> Result<WriteResult<Self::Entry>, Self::Error>
    where
        Self::Entry: VersionedCacheEntry<Self>,
    {
        debug_assert_eq!(metadata.cache_version(), expected_version + 1);
        write_at_version(self, input, metadata, expected_version).await
    }

    /// Like [`Self::try_write`] but keeps the version unchanged (`metadata`
    /// must carry `expected_version`). Used to persist refreshed bookkeeping
    /// for an entry that is otherwise unchanged, without conflicting
    /// concurrent writers out of the version CAS.
    async fn try_refresh(
        &self,
        input: &Self::Key,
        metadata: &Self::Entry,
        expected_version: u64,
    ) -> Result<WriteResult<Self::Entry>, Self::Error>
    where
        Self::Entry: VersionedCacheEntry<Self>,
    {
        debug_assert_eq!(metadata.cache_version(), expected_version);
        write_at_version(self, input, metadata, expected_version).await
    }

    /// Returns the path to the cache entry with the given key.
    fn cache_file_path(&self, input: &Self::Key) -> PathBuf {
        // Use string concatenation instead of `with_extension` to avoid issues
        // with dots in the key (e.g., from package names like "my.package").
        // `with_extension` replaces everything after the last dot, which would
        // truncate the file name.
        self.root().join(format!("{}.json", input.key()))
    }
}

/// Writes `metadata` if the stored entry still carries `expected_version`.
/// The lock is held only for the version check and the write.
async fn write_at_version<C: MetadataCache>(
    cache: &C,
    input: &C::Key,
    metadata: &C::Entry,
    expected_version: u64,
) -> Result<WriteResult<C::Entry>, C::Error>
where
    C::Entry: VersionedCacheEntry<C>,
{
    let cache_file_path = cache.cache_file_path(input);
    if let Some(parent) = cache_file_path.parent() {
        tokio::fs::create_dir_all(&parent).await.map_err(|e| {
            C::Error::from_io_error(
                "creating cache directory".to_string(),
                parent.to_path_buf(),
                e,
            )
        })?;
    }

    // Open or create the cache file
    let cache_file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&cache_file_path)
        .await
        .map_err(|e| {
            C::Error::from_io_error("opening cache file".to_string(), cache_file_path.clone(), e)
        })?;

    // Acquire lock
    let mut locked_cache_file = cache_file.lock_write().await.map_err(|e| {
        C::Error::from_io_error(
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
            C::Error::from_io_error("reading cache file".to_string(), cache_file_path.clone(), e)
        })?;

    // If cache exists and has different version, return conflict
    if !current_contents.is_empty()
        && let Ok(current_metadata) = serde_json::from_str::<C::Entry>(&current_contents)
        && current_metadata.cache_version() != expected_version
    {
        // Cache was updated by another process
        drop(locked_cache_file);
        return Ok(WriteResult::Conflict(current_metadata));
    }

    // Version matches (or cache is empty), write new data
    let bytes = serde_json::to_vec(metadata).expect("serialization to JSON should not fail");

    // Write to file
    locked_cache_file.rewind().await.map_err(|e| {
        C::Error::from_io_error(
            "seeking to start of cache file".to_string(),
            cache_file_path.clone(),
            e,
        )
    })?;

    locked_cache_file.write_all(&bytes).await.map_err(|e| {
        C::Error::from_io_error(
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
            C::Error::from_io_error(
                "setting length of cache file".to_string(),
                cache_file_path.clone(),
                e,
            )
        })?;

    // Flush to ensure data is written
    locked_cache_file.flush().await.map_err(|e| {
        C::Error::from_io_error("flushing cache file".to_string(), cache_file_path, e)
    })?;

    // Release lock
    drop(locked_cache_file);

    Ok(WriteResult::Written)
}

/// Trait for cache keys that can produce a unique string used as the file name.
///
/// Implementations typically hash a subset of fields and combine them with
/// human-readable components (e.g. platform, package name) so that cache
/// files are easy to identify on disk.
pub trait MetadataCacheKey<C: MetadataCache> {
    /// Returns a unique, semi-human-readable key string.
    ///
    /// The returned value is used directly as the stem of the cache file
    /// name (with `.json` appended). It may contain path separators to
    /// create subdirectories.
    fn key(&self) -> CacheKeyString<C>;
}

/// Convenience alias for the key type of a [`MetadataCache`].
///
/// Allows writing `CacheKey<BuildBackendMetadataCache>` instead of the
/// concrete `BuildBackendMetadataCacheKey`.
pub type CacheKey<C> = <C as MetadataCache>::Key;

/// Convenience alias for the entry type of a [`MetadataCache`].
///
/// Allows writing `CacheEntry<BuildBackendMetadataCache>` instead of the
/// concrete `BuildBackendMetadataCacheEntry`.
pub type CacheEntry<C> = <C as MetadataCache>::Entry;

/// Trait for the data stored in a cache file.
///
/// Every cache entry must be serializable (for writing to disk) and carry a
/// [`CacheRevision`] that identifies its content. The revision allows
/// downstream caches to detect when the upstream data they depend on has
/// changed without needing to re-read and compare the full entry.
pub trait MetadataCacheEntry<C: MetadataCache>: Serialize + DeserializeOwned {
    /// Returns the revision of this cache entry.
    fn revision(&self) -> &CacheRevision<C>;
}

/// Extension of [`MetadataCacheEntry`] that adds optimistic locking.
///
/// The version is a monotonically increasing counter that is bumped on every
/// write. It is separate from [`CacheRevision`]: the version tracks *how
/// many times* the file was written (for conflict detection between
/// concurrent processes), while the revision tracks *what content* is stored
/// (for cross-cache staleness detection).
pub trait VersionedCacheEntry<C: MetadataCache>: MetadataCacheEntry<C> {
    /// Returns the version counter carried by the entry, which is what
    /// [`MetadataCache::try_write`] persists.
    fn cache_version(&self) -> u64;
}

/// The outcome of a [`MetadataCache::try_write`] call.
#[derive(Debug)]
pub enum WriteResult<M> {
    /// The entry was successfully written to disk.
    Written,
    /// Another process updated the cache file between our read and write.
    /// Contains the entry that was written by the other process.
    Conflict(M),
}

/// An opaque identifier representing a specific revision of a cache entry.
///
/// A revision changes only when the meaningful content of an entry changes,
/// allowing downstream caches to detect when their upstream data is stale.
/// For example, the source metadata cache stores the revision of the build
/// backend metadata it was derived from; if that revision no longer matches
/// the current build backend entry, the source metadata is stale.
///
/// The type parameter `C` prevents accidentally comparing revisions from
/// different caches at compile time.
pub struct CacheRevision<C: ?Sized>(String, PhantomData<C>);

impl<C: ?Sized> CacheRevision<C> {
    /// Generates a new unique revision.
    pub fn new() -> Self {
        Self(nanoid::nanoid!(), PhantomData)
    }
}

impl<C: ?Sized> Default for CacheRevision<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: ?Sized> Clone for CacheRevision<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<C: ?Sized> PartialEq for CacheRevision<C> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<C: ?Sized> Eq for CacheRevision<C> {}

impl<C: ?Sized> std::hash::Hash for CacheRevision<C> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<C: ?Sized> std::fmt::Debug for CacheRevision<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CacheRevision").field(&self.0).finish()
    }
}

impl<C: ?Sized> std::fmt::Display for CacheRevision<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<C: ?Sized> Serialize for CacheRevision<C> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, C: ?Sized> Deserialize<'de> for CacheRevision<C> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|s| Self(s, PhantomData))
    }
}

/// The string representation of a [`MetadataCacheKey`], as returned by
/// [`MetadataCacheKey::key`]. This is the stem of the cache file name and
/// can be used to locate the file on disk.
///
/// The type parameter `C` prevents accidentally mixing key strings from
/// different caches.
pub struct CacheKeyString<C: ?Sized>(String, PhantomData<C>);

impl<C: ?Sized> CacheKeyString<C> {
    /// Wraps a key string.
    pub fn new(key: String) -> Self {
        Self(key, PhantomData)
    }

    /// Returns the key string as a str.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<C: ?Sized> Clone for CacheKeyString<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<C: ?Sized> PartialEq for CacheKeyString<C> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<C: ?Sized> Eq for CacheKeyString<C> {}

impl<C: ?Sized> std::fmt::Debug for CacheKeyString<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("CacheKeyString").field(&self.0).finish()
    }
}

impl<C: ?Sized> std::fmt::Display for CacheKeyString<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<C: ?Sized> Serialize for CacheKeyString<C> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, C: ?Sized> Deserialize<'de> for CacheKeyString<C> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|s| Self(s, PhantomData))
    }
}

/// Trait for cache-specific error types.
///
/// Each [`MetadataCache`] implementation defines its own error enum. This
/// trait provides a common constructor so that the default `read`/`try_write`
/// implementations can produce errors without knowing the concrete type.
pub trait CacheError: std::error::Error + Sized {
    /// Wraps an I/O error with the name of the operation that failed and the
    /// path that was being accessed.
    fn from_io_error(operation: String, path: PathBuf, error: std::io::Error) -> Self;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyKey(String);

    impl MetadataCacheKey<DummyCache> for DummyKey {
        fn key(&self) -> CacheKeyString<DummyCache> {
            CacheKeyString::new(self.0.clone())
        }
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    struct DummyMetadata {
        revision: CacheRevision<DummyCache>,
        version: u64,
    }

    impl MetadataCacheEntry<DummyCache> for DummyMetadata {
        fn revision(&self) -> &CacheRevision<DummyCache> {
            &self.revision
        }
    }

    impl VersionedCacheEntry<DummyCache> for DummyMetadata {
        fn cache_version(&self) -> u64 {
            self.version
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("test error")]
    struct DummyError;

    impl CacheError for DummyError {
        fn from_io_error(_operation: String, _path: PathBuf, _error: std::io::Error) -> Self {
            DummyError
        }
    }

    #[derive(Clone)]
    struct DummyCache {
        root: PathBuf,
    }

    impl MetadataCache for DummyCache {
        type Key = DummyKey;
        type Entry = DummyMetadata;
        type Error = DummyError;
        const CACHE_SUFFIX: &'static str = "v0";
        fn root(&self) -> &Path {
            &self.root
        }
    }

    #[test]
    fn test_cache_file_path_with_dots_in_key() {
        let cache = DummyCache {
            root: PathBuf::from("/tmp/cache"),
        };

        // A key with dots (e.g., from package name "my.package") should NOT
        // have the part after the dot replaced by `with_extension`.
        let key = DummyKey("source-dir/my.package-osx-arm64-HASH".to_string());
        let path = cache.cache_file_path(&key);
        assert_eq!(
            path,
            PathBuf::from("/tmp/cache/source-dir/my.package-osx-arm64-HASH.json")
        );

        // A key without dots should also work correctly.
        let key = DummyKey("source-dir/my-package-osx-arm64-HASH".to_string());
        let path = cache.cache_file_path(&key);
        assert_eq!(
            path,
            PathBuf::from("/tmp/cache/source-dir/my-package-osx-arm64-HASH.json")
        );
    }

    fn entry(version: u64) -> DummyMetadata {
        DummyMetadata {
            revision: CacheRevision::new(),
            version,
        }
    }

    /// `try_refresh` persists bookkeeping without consuming the version, so a
    /// writer that read the entry before the refresh still wins its CAS. If
    /// the refresh bumped the version instead, the rebuild would be forced to
    /// conflict and throw away its work.
    #[tokio::test]
    async fn try_refresh_keeps_the_version_so_a_concurrent_writer_still_wins() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = DummyCache {
            root: tmp.path().to_path_buf(),
        };
        let key = DummyKey("entry".to_string());

        // A rebuild writes the initial entry at version 0.
        assert!(matches!(
            cache.try_write(&key, &entry(1), 0).await.unwrap(),
            WriteResult::Written
        ));
        let stored = cache.read(&key).await.unwrap().unwrap();
        assert_eq!(stored.cache_version(), 1);

        // A read path refreshes it in place.
        assert!(matches!(
            cache.try_refresh(&key, &stored, 1).await.unwrap(),
            WriteResult::Written
        ));
        assert_eq!(cache.read(&key).await.unwrap().unwrap().cache_version(), 1);

        // A writer holding the pre-refresh version still commits.
        assert!(matches!(
            cache.try_write(&key, &entry(2), 1).await.unwrap(),
            WriteResult::Written
        ));
        assert_eq!(cache.read(&key).await.unwrap().unwrap().cache_version(), 2);
    }

    /// A refresh computed against a version that has since moved on must not
    /// overwrite the newer entry.
    #[tokio::test]
    async fn try_refresh_conflicts_when_the_entry_moved_on() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = DummyCache {
            root: tmp.path().to_path_buf(),
        };
        let key = DummyKey("entry".to_string());

        cache.try_write(&key, &entry(1), 0).await.unwrap();
        cache.try_write(&key, &entry(2), 1).await.unwrap();
        assert_eq!(cache.read(&key).await.unwrap().unwrap().cache_version(), 2);

        let refreshed = cache.try_refresh(&key, &entry(1), 1).await.unwrap();
        match refreshed {
            WriteResult::Conflict(current) => assert_eq!(current.cache_version(), 2),
            WriteResult::Written => panic!("a stale refresh must not overwrite a newer entry"),
        }
        assert_eq!(cache.read(&key).await.unwrap().unwrap().cache_version(), 2);
    }
}
