//! This module contains the `GlobHashCache` struct which is used to cache the computation of glob hashes. This cache is an in-process cache
//! so it's purpose is to re-use computed hashes across multiple calls to the same glob hash computation for the same set of input files.
//! The input files are deemed not to change between calls.
use std::{
    collections::BTreeSet,
    convert::identity,
    hash::Hash,
    path::PathBuf,
    sync::{Arc, Weak},
};

use dashmap::{DashMap, Entry};
use tokio::sync::broadcast;

use super::{GlobHash, GlobHashError};

/// A key for the cache of glob hashes.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct GlobHashKey {
    /// The root directory of the glob patterns.
    root: PathBuf,
    /// The glob patterns.
    globs: BTreeSet<String>,
    /// Additional hash which should invalidate the cache if it changes.
    additional_hash: Option<Vec<u8>>,
}

impl GlobHashKey {
    /// Creates a new `GlobHashKey` from the given root directory and glob patterns.
    pub fn new(
        root: impl Into<PathBuf>,
        globs: BTreeSet<String>,
        additional_hash: Option<Vec<u8>>,
    ) -> Self {
        let mut root = root.into();
        // Ensure that `root` points to a directory, not a file.
        if root.is_file() {
            root = root.parent().expect("Root must be a directory").to_owned();
        }

        Self {
            root,
            globs,
            additional_hash,
        }
    }
}

#[derive(Debug)]
enum HashCacheEntry {
    /// The value is currently being computed.
    Pending(Weak<broadcast::Sender<GlobHash>>),

    /// We have a value for this key.
    Done(GlobHash),
}

/// An object that caches the computation of glob hashes. It deduplicates
/// requests for the same hash.
///
/// Its is safe and efficient to use this object from multiple threads.
#[derive(Debug, Default, Clone)]
pub struct GlobHashCache {
    cache: Arc<DashMap<GlobHashKey, HashCacheEntry>>,
}

impl GlobHashCache {
    /// Computes the input hash of the given key. If the hash is already in the
    /// cache, it will return the cached value. If the hash is not in the
    /// cache, it will compute the hash (deduplicating any request) and return
    /// it.
    pub async fn compute_hash(&self, key: GlobHashKey) -> Result<GlobHash, GlobHashError> {
        match self.cache.entry(key.clone()) {
            Entry::Vacant(entry) => {
                // Construct a channel over which we will be sending the result and store it in
                // the map. If another requests comes in for the same hash it will find this
                // entry.
                let (tx, _) = broadcast::channel(1);
                let tx = Arc::new(tx);
                let weak_tx = Arc::downgrade(&tx);
                entry.insert(HashCacheEntry::Pending(weak_tx));

                // Spawn the computation of the hash
                let computation_key = key.clone();
                let result = tokio::task::spawn_blocking(move || {
                    GlobHash::from_patterns(
                        &computation_key.root,
                        computation_key.globs.iter().map(String::as_str),
                        computation_key.additional_hash,
                    )
                })
                .await
                .map_or_else(
                    |err| match err.try_into_panic() {
                        Ok(panic) => std::panic::resume_unwind(panic),
                        Err(_) => Err(GlobHashError::Cancelled),
                    },
                    identity,
                )?;

                // Store the result in the cache
                self.cache.insert(key, HashCacheEntry::Done(result.clone()));

                // Broadcast the result, ignore the error. If the receiver is dropped, we don't
                // care.
                let _ = tx.send(result.clone());

                Ok(result)
            }
            Entry::Occupied(entry) => {
                match entry.get() {
                    HashCacheEntry::Pending(weak_tx) => {
                        let sender = weak_tx.clone();
                        let mut subscriber = sender
                            .upgrade()
                            .ok_or(GlobHashError::Cancelled)?
                            .subscribe();
                        drop(entry);
                        subscriber
                            .recv()
                            .await
                            .map_err(|_| GlobHashError::Cancelled)
                    }
                    HashCacheEntry::Done(hash) => {
                        // We have a value for this key.
                        Ok(hash.clone())
                    }
                }
            }
        }
    }
}
