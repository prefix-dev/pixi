use std::{path::PathBuf, sync::Mutex};

use http_cache_reqwest::{CacheManager, HttpResponse};
use http_cache_semantics::CachePolicy;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxError>;

/// A wrapper that stores both the HTTP response and its cache policy
/// together for serialization/deserialization.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    response: HttpResponse,
    policy: CachePolicy,
}

/// A [`CacheManager`] implementation backed by a SQLite database.
///
/// This replaces the default file-based [`CACacheManager`] to avoid creating
/// many small files on disk, which performs poorly on HPC and network
/// filesystems. Instead, all cached HTTP responses are stored in a single
/// SQLite database file.
///
/// The database uses WAL journal mode for good concurrent read performance
/// and sets `synchronous = NORMAL` since this is a cache and data loss on
/// crash is acceptable.
pub struct SqliteCacheManager {
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteCacheManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteCacheManager").finish()
    }
}

impl SqliteCacheManager {
    /// Creates a new [`SqliteCacheManager`] that stores cache data in the given
    /// SQLite database file path.
    ///
    /// The parent directory will be created if it does not exist. The database
    /// is configured with WAL journal mode and relaxed sync for performance.
    pub fn new(path: PathBuf) -> Result<Self> {
        // Ensure the parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(&path)?;

        // WAL mode allows concurrent readers and a single writer, which is
        // much better for our use-case than the default rollback journal.
        connection.pragma_update(None, "journal_mode", "WAL")?;

        // Since this is a cache, we can afford to lose data on a crash.
        // NORMAL sync is significantly faster than FULL.
        connection.pragma_update(None, "synchronous", "NORMAL")?;

        // Set a busy timeout so concurrent processes wait rather than
        // immediately failing with SQLITE_BUSY.
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        // Create the cache table if it doesn't exist.
        connection.execute(
            "CREATE TABLE IF NOT EXISTS http_cache (
                cache_key TEXT PRIMARY KEY,
                data BLOB NOT NULL
            )",
            [],
        )?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

#[async_trait::async_trait]
impl CacheManager for SqliteCacheManager {
    async fn get(&self, cache_key: &str) -> Result<Option<(HttpResponse, CachePolicy)>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| -> BoxError { format!("mutex poisoned: {e}").into() })?;
        let mut stmt = conn.prepare_cached("SELECT data FROM http_cache WHERE cache_key = ?1")?;
        let result: Option<Vec<u8>> = stmt
            .query_row(params![cache_key], |row| row.get(0))
            .optional()?;

        match result {
            Some(data) => {
                let entry: CacheEntry = serde_json::from_slice(&data)?;
                Ok(Some((entry.response, entry.policy)))
            }
            None => Ok(None),
        }
    }

    async fn put(
        &self,
        cache_key: String,
        response: HttpResponse,
        policy: CachePolicy,
    ) -> Result<HttpResponse> {
        let entry = CacheEntry { response, policy };
        let data = serde_json::to_vec(&entry)?;
        let conn = self
            .connection
            .lock()
            .map_err(|e| -> BoxError { format!("mutex poisoned: {e}").into() })?;
        conn.execute(
            "INSERT OR REPLACE INTO http_cache (cache_key, data) VALUES (?1, ?2)",
            params![cache_key, data],
        )?;
        Ok(entry.response)
    }

    async fn delete(&self, cache_key: &str) -> Result<()> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| -> BoxError { format!("mutex poisoned: {e}").into() })?;
        conn.execute(
            "DELETE FROM http_cache WHERE cache_key = ?1",
            params![cache_key],
        )?;
        Ok(())
    }
}
