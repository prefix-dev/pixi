use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use http_cache_reqwest::{CacheManager, HttpResponse};
use http_cache_semantics::CachePolicy;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxError>;

/// The response metadata stored as JSON — everything except the raw body.
#[derive(Debug, Serialize, Deserialize)]
struct ResponseMeta {
    headers: HashMap<String, String>,
    status: u16,
    url: url::Url,
    version: HttpVersion,
}

/// Mirror of `http_cache::HttpVersion` so we can serialize the metadata
/// independently of the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum HttpVersion {
    #[serde(rename = "HTTP/0.9")]
    Http09,
    #[serde(rename = "HTTP/1.0")]
    Http10,
    #[serde(rename = "HTTP/1.1")]
    Http11,
    #[serde(rename = "HTTP/2.0")]
    H2,
    #[serde(rename = "HTTP/3.0")]
    H3,
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
///
/// The schema stores the response body as a raw BLOB (no serialization
/// overhead), while the response metadata (headers, status, url, version) and
/// the cache policy are stored as separate JSON columns.
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
                body BLOB NOT NULL,
                response_meta TEXT NOT NULL,
                policy TEXT NOT NULL
            )",
            [],
        )?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

/// Reconstruct an [`HttpResponse`] by round-tripping through our local
/// [`ResponseMeta`] + [`HttpVersion`] types.
///
/// Both our local types and the upstream types use identical serde `rename`
/// attributes, so a JSON round-trip through `serde_json::Value` is the
/// simplest way to convert without depending on private upstream fields.
fn response_from_parts(body: Vec<u8>, meta: ResponseMeta) -> Result<HttpResponse> {
    let mut map = serde_json::to_value(&meta)?;
    map.as_object_mut()
        .ok_or("expected JSON object")?
        .insert("body".to_string(), serde_json::to_value(&body)?);
    let response: HttpResponse = serde_json::from_value(map)?;
    Ok(response)
}

fn response_to_parts(response: &HttpResponse) -> Result<(Vec<u8>, String)> {
    // Serialize the full response to a JSON value, then pull out the body
    // and keep the rest as metadata.
    let mut map = serde_json::to_value(response)?;
    let obj = map.as_object_mut().ok_or("expected JSON object")?;
    obj.remove("body");
    let meta_json = serde_json::to_string(&map)?;
    Ok((response.body.clone(), meta_json))
}

#[async_trait::async_trait]
impl CacheManager for SqliteCacheManager {
    async fn get(&self, cache_key: &str) -> Result<Option<(HttpResponse, CachePolicy)>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| -> BoxError { format!("mutex poisoned: {e}").into() })?;
        let mut stmt = conn.prepare_cached(
            "SELECT body, response_meta, policy FROM http_cache WHERE cache_key = ?1",
        )?;
        let result = stmt
            .query_row(params![cache_key], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()?;

        match result {
            Some((body, meta_json, policy_json)) => {
                let meta: ResponseMeta = serde_json::from_str(&meta_json)?;
                let policy: CachePolicy = serde_json::from_str(&policy_json)?;
                let response = response_from_parts(body, meta)?;
                Ok(Some((response, policy)))
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
        let (body, meta_json) = response_to_parts(&response)?;
        let policy_json = serde_json::to_string(&policy)?;
        let conn = self
            .connection
            .lock()
            .map_err(|e| -> BoxError { format!("mutex poisoned: {e}").into() })?;
        conn.execute(
            "INSERT OR REPLACE INTO http_cache (cache_key, body, response_meta, policy) VALUES (?1, ?2, ?3, ?4)",
            params![cache_key, body, meta_json, policy_json],
        )?;
        Ok(response)
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

#[cfg(test)]
mod tests {
    use super::*;

    use http_cache_semantics::CachePolicy;
    use std::time::SystemTime;

    /// Build a minimal [`HttpResponse`] with the given body bytes.
    fn make_response(body: &[u8], status: u16) -> HttpResponse {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "application/octet-stream".to_string(),
        );
        let value = serde_json::json!({
            "body": body,
            "headers": headers,
            "status": status,
            "url": "https://example.com/test",
            "version": "HTTP/1.1",
        });
        serde_json::from_value(value).expect("valid HttpResponse")
    }

    /// Build a [`CachePolicy`] from a simple GET request / response pair.
    fn make_policy() -> CachePolicy {
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/test")
            .body(())
            .unwrap();
        let res = http::Response::builder()
            .status(200)
            .header("cache-control", "max-age=3600")
            .body(())
            .unwrap();
        CachePolicy::new_options(&req, &res, SystemTime::now(), Default::default())
    }

    /// Helper: create a [`SqliteCacheManager`] backed by a temp file.
    fn temp_manager() -> (SqliteCacheManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cache.sqlite");
        let manager = SqliteCacheManager::new(db_path).unwrap();
        (manager, dir)
    }

    #[tokio::test]
    async fn get_missing_key_returns_none() {
        let (manager, _dir) = temp_manager();
        let result = manager.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn put_then_get_roundtrips() {
        let (manager, _dir) = temp_manager();
        let body = b"hello world";
        let response = make_response(body, 200);
        let policy = make_policy();

        manager
            .put("key1".to_string(), response, policy)
            .await
            .unwrap();

        let (cached_response, _cached_policy) = manager.get("key1").await.unwrap().unwrap();
        assert_eq!(cached_response.body, body);
        assert_eq!(cached_response.status, 200);
    }

    #[tokio::test]
    async fn put_overwrites_existing_entry() {
        let (manager, _dir) = temp_manager();

        let response_v1 = make_response(b"version1", 200);
        manager
            .put("key".to_string(), response_v1, make_policy())
            .await
            .unwrap();

        let response_v2 = make_response(b"version2", 201);
        manager
            .put("key".to_string(), response_v2, make_policy())
            .await
            .unwrap();

        let (cached, _) = manager.get("key").await.unwrap().unwrap();
        assert_eq!(cached.body, b"version2");
        assert_eq!(cached.status, 201);
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let (manager, _dir) = temp_manager();

        manager
            .put(
                "key".to_string(),
                make_response(b"data", 200),
                make_policy(),
            )
            .await
            .unwrap();
        assert!(manager.get("key").await.unwrap().is_some());

        manager.delete("key").await.unwrap();
        assert!(manager.get("key").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_key_is_ok() {
        let (manager, _dir) = temp_manager();
        // Should not error when deleting a key that doesn't exist.
        manager.delete("ghost").await.unwrap();
    }

    #[tokio::test]
    async fn multiple_keys_are_independent() {
        let (manager, _dir) = temp_manager();

        manager
            .put("a".to_string(), make_response(b"alpha", 200), make_policy())
            .await
            .unwrap();
        manager
            .put("b".to_string(), make_response(b"beta", 201), make_policy())
            .await
            .unwrap();

        let (a, _) = manager.get("a").await.unwrap().unwrap();
        let (b, _) = manager.get("b").await.unwrap().unwrap();
        assert_eq!(a.body, b"alpha");
        assert_eq!(b.body, b"beta");
        assert_eq!(a.status, 200);
        assert_eq!(b.status, 201);

        manager.delete("a").await.unwrap();
        assert!(manager.get("a").await.unwrap().is_none());
        assert!(manager.get("b").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn preserves_response_headers() {
        let (manager, _dir) = temp_manager();

        let mut headers = HashMap::new();
        headers.insert("x-custom".to_string(), "value123".to_string());
        headers.insert("content-type".to_string(), "text/plain".to_string());
        let value = serde_json::json!({
            "body": b"hi".to_vec(),
            "headers": headers,
            "status": 200,
            "url": "https://example.com/headers",
            "version": "HTTP/1.1",
        });
        let response: HttpResponse = serde_json::from_value(value).unwrap();

        manager
            .put("hdr".to_string(), response, make_policy())
            .await
            .unwrap();

        let (cached, _) = manager.get("hdr").await.unwrap().unwrap();
        assert_eq!(
            cached.headers.get("x-custom").map(String::as_str),
            Some("value123")
        );
        assert_eq!(
            cached.headers.get("content-type").map(String::as_str),
            Some("text/plain")
        );
    }

    #[tokio::test]
    async fn handles_binary_body() {
        let (manager, _dir) = temp_manager();

        // Non-UTF8 bytes including null bytes
        let binary: Vec<u8> = (0..=255).collect();
        let response = make_response(&binary, 200);

        manager
            .put("bin".to_string(), response, make_policy())
            .await
            .unwrap();

        let (cached, _) = manager.get("bin").await.unwrap().unwrap();
        assert_eq!(cached.body, binary);
    }

    #[tokio::test]
    async fn handles_empty_body() {
        let (manager, _dir) = temp_manager();

        let response = make_response(b"", 204);
        manager
            .put("empty".to_string(), response, make_policy())
            .await
            .unwrap();

        let (cached, _) = manager.get("empty").await.unwrap().unwrap();
        assert!(cached.body.is_empty());
        assert_eq!(cached.status, 204);
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cache.sqlite");

        {
            let manager = SqliteCacheManager::new(db_path.clone()).unwrap();
            manager
                .put(
                    "persist".to_string(),
                    make_response(b"durable", 200),
                    make_policy(),
                )
                .await
                .unwrap();
            // manager is dropped here, closing the connection
        }

        // Re-open the same database
        let manager2 = SqliteCacheManager::new(db_path).unwrap();
        let (cached, _) = manager2.get("persist").await.unwrap().unwrap();
        assert_eq!(cached.body, b"durable");
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir
            .path()
            .join("deeply")
            .join("nested")
            .join("cache.sqlite");
        let manager = SqliteCacheManager::new(db_path).unwrap();

        manager
            .put(
                "ok".to_string(),
                make_response(b"nested", 200),
                make_policy(),
            )
            .await
            .unwrap();

        let (cached, _) = manager.get("ok").await.unwrap().unwrap();
        assert_eq!(cached.body, b"nested");
    }
}
