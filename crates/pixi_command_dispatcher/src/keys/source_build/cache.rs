//! Artifact cache for source builds.
//!
//! Layout (under `<cache_root>/source_builds/artifacts/`):
//!
//! ```text
//! artifacts/<package_name>/<cache_key>/
//!     <pkg>-<ver>-<build>.conda
//!     sidecar.json
//! ```
//!
//! The cache key is structural (identity of the build inputs plus content
//! addresses of its dependencies). Freshness of the source files themselves
//! is tracked in the sidecar via per-file mtimes, plus a re-glob pass that
//! detects added files.
//!
//! On a hit, the cached `.conda` is returned along with its sha256; no
//! backend work happens. On a miss (or stale sidecar), the caller rebuilds
//! and calls [`ArtifactCache::store`] to populate the entry.
//!
//! Failed builds are never persisted; the compute engine caches failures in
//! memory for the lifetime of the process.

use std::{
    collections::{BTreeMap, BinaryHeap},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_fd_lock::{LockRead, LockWrite};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use pixi_glob::GlobSet;
use pixi_record::{UnresolvedPixiRecord, UnresolvedSourceRecord};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

/// Opaque content-addressed handle for an artifact cache entry.
///
/// Format: `<host_platform>-<xxh3-base64url>`. The leading platform makes the
/// directory name human-scannable; the xxh3 suffix provides the actual
/// collision resistance. Package name is not included because the entry
/// lives under a `<package_name>/` parent directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactCacheKey(String);

impl std::fmt::Display for ArtifactCacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Compute the artifact cache key for a source build.
///
/// Inputs that go into the hash:
/// - package name, pinned manifest source, pinned build source, variants
/// - build + host platform
/// - backend identifier (version + name of the build backend)
/// - url + sha256 of every binary dep in `build_packages` / `host_packages`,
///   tagged by bucket so a dep moving build ↔ host invalidates
/// - sha256 of every source dep artifact, also tagged by bucket
///
/// Source *files* are not hashed here: the sidecar captures their mtimes
/// separately so a content change still invalidates the entry on lookup.
///
/// The caller provides source-dep sha256s split into build / host buckets
/// to preserve the same bucket separation applied to binary deps.
pub fn compute_artifact_cache_key(
    record: &UnresolvedSourceRecord,
    build_platform: Platform,
    host_platform: Platform,
    backend_identifier: &str,
    build_source_dep_sha256s: &[Sha256Hash],
    host_source_dep_sha256s: &[Sha256Hash],
) -> ArtifactCacheKey {
    let mut hasher = Xxh3::new();
    record.name().as_normalized().hash(&mut hasher);
    record.manifest_source.hash(&mut hasher);
    record.build_source.hash(&mut hasher);
    record.variants.hash(&mut hasher);
    build_platform.hash(&mut hasher);
    host_platform.hash(&mut hasher);
    backend_identifier.hash(&mut hasher);

    // Bucket-tagged streams: the same (url, sha256) behaves differently
    // when installed into the build prefix vs. the host prefix because
    // run-dep resolution uses different pin-compatibility maps. Hash a
    // distinct marker per bucket so the two cases can never collide.
    "build_packages".hash(&mut hasher);
    for dep in &record.build_packages {
        if let UnresolvedPixiRecord::Binary(repo) = dep {
            repo.url.as_str().hash(&mut hasher);
            repo.package_record.sha256.hash(&mut hasher);
        }
    }
    for sha in build_source_dep_sha256s {
        sha.hash(&mut hasher);
    }

    "host_packages".hash(&mut hasher);
    for dep in &record.host_packages {
        if let UnresolvedPixiRecord::Binary(repo) = dep {
            repo.url.as_str().hash(&mut hasher);
            repo.package_record.sha256.hash(&mut hasher);
        }
    }
    for sha in host_source_dep_sha256s {
        sha.hash(&mut hasher);
    }

    let hash = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
    ArtifactCacheKey(format!("{host_platform}-{hash}"))
}

/// On-disk record that lives next to a cached `.conda`.
///
/// Format has no version field by design: a parse failure triggers cache
/// invalidation, and fields are only ever added in a serde-backwards-compatible
/// way (new fields get `#[serde(default)]`).
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSidecar {
    /// Glob patterns that match the set of files the build reads. Used at
    /// lookup time to detect newly-added matching files.
    #[serde(default, skip_serializing_if = "BinaryHeap::is_empty")]
    pub input_globs: BinaryHeap<String>,

    /// Paths of the files actually read by the build, relative to the source
    /// directory, paired with their mtime at build time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_files: BTreeMap<PathBuf, DateTime<Utc>>,

    /// sha256 of the cached `.conda`. Callers rely on this for transitive
    /// invalidation of dependents.
    #[serde_as(as = "rattler_digest::serde::SerializableHash<rattler_digest::Sha256>")]
    pub artifact_sha256: Sha256Hash,

    /// File name of the cached `.conda` within the entry directory.
    pub artifact_filename: String,

    /// Fully-assembled `RepoDataRecord` for the artifact. Stored so cache
    /// hits can return it without re-reading index.json from the `.conda`.
    pub record: RepoDataRecord,
}

/// Result of a successful cache lookup.
#[derive(Debug, Clone)]
pub struct CachedArtifact {
    pub artifact: PathBuf,
    pub sha256: Sha256Hash,
    pub record: RepoDataRecord,
}

/// Artifact cache rooted at `<cache_root>/artifacts/`.
#[derive(Clone, Debug)]
pub struct ArtifactCache {
    root: PathBuf,
}

impl ArtifactCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Directory that holds every entry for `package`.
    pub fn package_dir(&self, package: &PackageName) -> PathBuf {
        self.root.join(package.as_normalized())
    }

    /// Directory that holds the entry for `(package, key)`.
    pub fn entry_dir(&self, package: &PackageName, key: &ArtifactCacheKey) -> PathBuf {
        self.package_dir(package).join(&key.0)
    }

    fn sidecar_path(&self, package: &PackageName, key: &ArtifactCacheKey) -> PathBuf {
        self.entry_dir(package, key).join("sidecar.json")
    }

    /// Remove every cached entry for `package`. Silently succeeds if
    /// there's nothing cached yet. Intended for CLI invalidation
    /// commands like `pixi build --clean` or `pixi install
    /// --force-reinstall`.
    pub fn clear_package(&self, package: &PackageName) -> std::io::Result<()> {
        let dir = self.package_dir(package);
        match fs_err::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Remove every cached entry across every package.
    pub fn clear_all(&self) -> std::io::Result<()> {
        match fs_err::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Path to the entry's cross-process `.lock` file. Callers hold a
    /// read lock on this file while inspecting the sidecar and a write
    /// lock while storing the artifact + sidecar. The file sits inside
    /// the entry directory so it gets removed alongside the entry when
    /// [`clear_package`](Self::clear_package) runs.
    fn lock_path(&self, package: &PackageName, key: &ArtifactCacheKey) -> PathBuf {
        self.entry_dir(package, key).join(".lock")
    }

    /// Open (creating if needed) the entry's lock file.
    async fn open_lock_file(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
    ) -> Result<tokio::fs::File, ArtifactCacheError> {
        let entry_dir = self.entry_dir(package, key);
        tokio::fs::create_dir_all(&entry_dir).await.map_err(|err| {
            ArtifactCacheError::io("creating entry directory", entry_dir.clone(), err)
        })?;
        let lock_path = self.lock_path(package, key);
        tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .await
            .map_err(|err| ArtifactCacheError::io("opening lock file", lock_path, err))
    }

    /// Look up an entry. Returns `Ok(None)` if the entry is missing or
    /// stale (parse failure, mtime mismatch, missing file, or a newly-added
    /// file matches the recorded globs).
    ///
    /// Holds a shared read lock on the entry's `.lock` file while
    /// reading the sidecar, so a concurrent [`store`](Self::store) (which
    /// takes an exclusive write lock) blocks until we're done. The lock
    /// is dropped before the mtime/glob checks run, since those only touch
    /// files outside the cache entry and don't race with writers.
    pub async fn lookup(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
        source_dir: &Path,
    ) -> Result<Option<CachedArtifact>, ArtifactCacheError> {
        let sidecar_path = self.sidecar_path(package, key);
        // Fast-path the common "no entry yet" case: skip creating the
        // lock file at all when the sidecar doesn't exist. If the
        // entry directory was never created, neither was the lock.
        if fs_err::metadata(&sidecar_path).is_err() {
            return Ok(None);
        }

        let lock_file = self.open_lock_file(package, key).await?;
        let _guard = lock_file.lock_read().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring shared lock",
                self.lock_path(package, key),
                err.error,
            )
        })?;

        let bytes = match tokio::fs::read(&sidecar_path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(ArtifactCacheError::io("reading sidecar", sidecar_path, err)),
        };
        let Ok(sidecar) = serde_json::from_slice::<ArtifactSidecar>(&bytes) else {
            // Treat unparsable sidecars as misses; the caller will rebuild
            // and overwrite.
            return Ok(None);
        };
        drop(_guard);

        // Check every recorded file still matches its mtime.
        for (rel, expected_mtime) in &sidecar.input_files {
            let full = source_dir.join(rel);
            let modified = match fs_err::metadata(&full).and_then(|m| m.modified()) {
                Ok(m) => DateTime::<Utc>::from(m),
                Err(_) => return Ok(None),
            };
            if modified != *expected_mtime {
                return Ok(None);
            }
        }

        // Detect newly-added files that match the stored globs. This catches
        // sources added after the cache entry was written.
        if !sidecar.input_globs.is_empty() {
            let glob_set = GlobSet::create(sidecar.input_globs.iter().map(String::as_str));
            let matches = glob_set
                .collect_matching(source_dir)
                .map_err(ArtifactCacheError::from)?;
            for matched in matches {
                let path = matched.into_path();
                let rel = path.strip_prefix(source_dir).unwrap_or(&path).to_path_buf();
                if !sidecar.input_files.contains_key(&rel) {
                    return Ok(None);
                }
            }
        }

        let artifact_path = self
            .entry_dir(package, key)
            .join(&sidecar.artifact_filename);
        if fs_err::metadata(&artifact_path).is_err() {
            return Ok(None);
        }

        Ok(Some(CachedArtifact {
            artifact: artifact_path,
            sha256: sidecar.artifact_sha256,
            record: sidecar.record,
        }))
    }

    /// Place `artifact_source` into the cache and write its sidecar.
    ///
    /// `input_files` are paths relative to `source_dir`; their mtimes are
    /// captured at store time. `record` is the synthesized `RepoDataRecord`
    /// for the artifact; it is persisted in the sidecar so cache hits can
    /// skip re-reading index.json.
    ///
    /// Holds an exclusive write lock on the entry's `.lock` file for
    /// the artifact copy + sidecar write, so a concurrent
    /// [`lookup`](Self::lookup) blocks until the new state is fully
    /// committed.
    #[allow(clippy::too_many_arguments)]
    pub async fn store(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
        artifact_source: &Path,
        input_globs: impl IntoIterator<Item = String>,
        input_files: impl IntoIterator<Item = PathBuf>,
        source_dir: &Path,
        record: RepoDataRecord,
    ) -> Result<CachedArtifact, ArtifactCacheError> {
        let entry_dir = self.entry_dir(package, key);
        let lock_file = self.open_lock_file(package, key).await?;
        let _guard = lock_file.lock_write().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring exclusive lock",
                self.lock_path(package, key),
                err.error,
            )
        })?;

        let filename = artifact_source
            .file_name()
            .ok_or_else(|| ArtifactCacheError::ArtifactFilename(artifact_source.to_path_buf()))?
            .to_string_lossy()
            .into_owned();
        let dest = entry_dir.join(&filename);

        // Copy rather than move: the source may live outside our cache root
        // (e.g. a backend-managed work directory), and a copy keeps failure
        // semantics simple even across filesystems.
        tokio::fs::copy(artifact_source, &dest)
            .await
            .map_err(|err| {
                ArtifactCacheError::io("copying artifact into cache", dest.clone(), err)
            })?;

        let sha256 = {
            let path = dest.clone();
            tokio::task::spawn_blocking(move || {
                rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&path)
            })
            .await
            .expect("sha256 task panicked")
            .map_err(|err| ArtifactCacheError::io("hashing artifact", dest.clone(), err))?
        };

        let mut input_files_mtimes = BTreeMap::new();
        for rel in input_files {
            let full = source_dir.join(&rel);
            let modified = fs_err::metadata(&full)
                .and_then(|m| m.modified())
                .map_err(|err| ArtifactCacheError::io("stat input file", full.clone(), err))?;
            input_files_mtimes.insert(rel, DateTime::<Utc>::from(modified));
        }

        let sidecar = ArtifactSidecar {
            input_globs: input_globs.into_iter().collect(),
            input_files: input_files_mtimes,
            artifact_sha256: sha256,
            artifact_filename: filename,
            record: record.clone(),
        };
        let sidecar_path = self.sidecar_path(package, key);
        let bytes = serde_json::to_vec(&sidecar).expect("sidecar serialization cannot fail");
        tokio::fs::write(&sidecar_path, &bytes)
            .await
            .map_err(|err| ArtifactCacheError::io("writing sidecar", sidecar_path.clone(), err))?;

        Ok(CachedArtifact {
            artifact: dest,
            sha256,
            record,
        })
    }
}

#[derive(Debug, Error, Clone)]
pub enum ArtifactCacheError {
    #[error("{operation} at {}", path.display())]
    Io {
        operation: String,
        path: PathBuf,
        #[source]
        source: Arc<std::io::Error>,
    },

    #[error(transparent)]
    Glob(Arc<pixi_glob::GlobSetError>),

    #[error("artifact path has no filename: {}", .0.display())]
    ArtifactFilename(PathBuf),
}

impl ArtifactCacheError {
    fn io(operation: impl Into<String>, path: PathBuf, err: std::io::Error) -> Self {
        Self::Io {
            operation: operation.into(),
            path,
            source: Arc::new(err),
        }
    }
}

impl From<pixi_glob::GlobSetError> for ArtifactCacheError {
    fn from(err: pixi_glob::GlobSetError) -> Self {
        Self::Glob(Arc::new(err))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{PackageName, PackageRecord};
    use tempfile::TempDir;

    use super::*;

    fn key(s: &str) -> ArtifactCacheKey {
        ArtifactCacheKey(s.to_string())
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::from_str(name).unwrap()
    }

    fn dummy_record(name: &str) -> RepoDataRecord {
        let mut pr = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        RepoDataRecord {
            package_record: pr,
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                &format!("{name}-1.0.0-h0.conda"),
            )
            .unwrap(),
            url: url::Url::parse(&format!("file:///{name}-1.0.0-h0.conda")).unwrap(),
            channel: None,
        }
    }

    #[tokio::test]
    async fn lookup_missing_entry_returns_none() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let got = cache
            .lookup(&pkg("foo"), &key("linux-64-abc"), &source)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn round_trip_store_then_lookup() {
        let tmp = TempDir::new().unwrap();
        let cache_root = tmp.path().join("artifacts");
        let cache = ArtifactCache::new(&cache_root);

        // A fake source tree with one input file.
        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let input = source.join("main.py");
        fs_err::write(&input, b"print(1)").unwrap();

        // A fake .conda artifact somewhere outside the cache.
        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"pretend this is a conda").unwrap();

        let key = key("linux-64-abc");
        let stored = cache
            .store(
                &pkg("foo"),
                &key,
                &artifact,
                vec!["**/*.py".to_string()],
                vec![PathBuf::from("main.py")],
                &source,
                dummy_record("foo"),
            )
            .await
            .unwrap();

        let hit = cache.lookup(&pkg("foo"), &key, &source).await.unwrap();
        let hit = hit.expect("cache hit after store");
        assert_eq!(hit.sha256, stored.sha256);
        assert_eq!(hit.artifact, stored.artifact);
        assert_eq!(
            hit.record.package_record.name,
            stored.record.package_record.name
        );
    }

    #[tokio::test]
    async fn lookup_stale_when_source_file_changes() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let input = source.join("main.py");
        fs_err::write(&input, b"old").unwrap();

        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"artifact").unwrap();

        let key = key("linux-64-abc");
        cache
            .store(
                &pkg("foo"),
                &key,
                &artifact,
                Vec::<String>::new(),
                vec![PathBuf::from("main.py")],
                &source,
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Modify the source file's mtime by rewriting it after a sleep.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs_err::write(&input, b"new").unwrap();

        let got = cache.lookup(&pkg("foo"), &key, &source).await.unwrap();
        assert!(got.is_none(), "mtime change should invalidate the entry");
    }

    #[tokio::test]
    async fn lookup_stale_when_new_file_matches_glob() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        fs_err::write(source.join("main.py"), b"body").unwrap();

        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"artifact").unwrap();

        let key = key("linux-64-abc");
        cache
            .store(
                &pkg("foo"),
                &key,
                &artifact,
                vec!["**/*.py".to_string()],
                vec![PathBuf::from("main.py")],
                &source,
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Introduce a new file that matches the stored glob.
        fs_err::write(source.join("extra.py"), b"new module").unwrap();

        let got = cache.lookup(&pkg("foo"), &key, &source).await.unwrap();
        assert!(
            got.is_none(),
            "a newly-added matching file should invalidate the entry"
        );
    }

    #[tokio::test]
    async fn sidecar_preserves_record_fields_across_lookup() {
        // Verify that every RepoDataRecord field we care about
        // (identifier, url, package_record.version, .build, .subdir,
        // .depends) round-trips through the sidecar. A future refactor
        // that accidentally drops a field will fail this test.
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"pretend-conda").unwrap();

        let mut record = dummy_record("foo");
        record.package_record.depends = vec!["python >=3.8".into(), "numpy".into()];
        record.package_record.build_number = 42;
        record.url = url::Url::parse("https://example.test/foo-1.0.0-h0.conda").unwrap();

        let key = key("linux-64-abc");
        cache
            .store(
                &pkg("foo"),
                &key,
                &artifact,
                Vec::<String>::new(),
                Vec::<PathBuf>::new(),
                &source,
                record.clone(),
            )
            .await
            .unwrap();

        let hit = cache
            .lookup(&pkg("foo"), &key, &source)
            .await
            .unwrap()
            .expect("cache should hit");

        // Package record identity.
        assert_eq!(hit.record.package_record.name, record.package_record.name);
        assert_eq!(
            hit.record.package_record.version,
            record.package_record.version
        );
        assert_eq!(hit.record.package_record.build, record.package_record.build);
        assert_eq!(
            hit.record.package_record.subdir,
            record.package_record.subdir
        );
        assert_eq!(hit.record.package_record.build_number, 42);
        assert_eq!(
            hit.record.package_record.depends,
            record.package_record.depends
        );

        // RepoDataRecord top-level fields.
        assert_eq!(hit.record.url, record.url);
        assert_eq!(hit.record.identifier, record.identifier);
    }

    /// A second `store` concurrent with a `lookup` must see a consistent
    /// view: either the pre-existing entry or the freshly-written one,
    /// never a mix of the two. We can't easily assert interleaving here
    /// in a deterministic way, so we smoke-test the lock path by
    /// spawning a lot of concurrent lookups + stores and asserting no
    /// IO errors fall out.
    #[tokio::test]
    async fn concurrent_store_and_lookup_do_not_error() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        fs_err::write(source.join("main.py"), b"body").unwrap();
        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"pretend").unwrap();

        let key = key("linux-64-abc");
        let pkg = pkg("foo");

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache = cache.clone();
            let source = source.clone();
            let artifact = artifact.clone();
            let pkg = pkg.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                cache
                    .store(
                        &pkg,
                        &key,
                        &artifact,
                        Vec::<String>::new(),
                        vec![PathBuf::from("main.py")],
                        &source,
                        dummy_record("foo"),
                    )
                    .await
                    .unwrap();
            }));
        }
        for _ in 0..8 {
            let cache = cache.clone();
            let source = source.clone();
            let pkg = pkg.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                // Ignore whether it's a hit or miss; racing with stores
                // the lookup may see either. The contract under test is
                // that it never errors out.
                let _ = cache.lookup(&pkg, &key, &source).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn corrupt_sidecar_is_a_miss() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();

        let entry = cache.entry_dir(&pkg("foo"), &key("linux-64-abc"));
        fs_err::create_dir_all(&entry).unwrap();
        fs_err::write(entry.join("sidecar.json"), b"{ this is not valid").unwrap();

        let got = cache
            .lookup(&pkg("foo"), &key("linux-64-abc"), &source)
            .await
            .unwrap();
        assert!(got.is_none());
    }
}

#[cfg(test)]
mod cache_key_tests {
    //! Every input to `compute_artifact_cache_key` must contribute to the
    //! key: equivalent inputs produce equal keys, any single-field change
    //! produces a different key. These tests are the guardrail against
    //! silent cache collisions when the hash input set evolves.
    use std::{collections::BTreeMap, str::FromStr, sync::Arc};

    use pixi_record::{
        FullSourceRecordData, PinnedPathSpec, PinnedSourceSpec, SourceRecordData,
        UnresolvedPixiRecord, UnresolvedSourceRecord,
    };
    use rattler_conda_types::{PackageName, PackageRecord, Platform, RepoDataRecord};
    use rattler_digest::{Sha256Hash, parse_digest_from_hex};
    use typed_path::Utf8TypedPathBuf;

    use super::compute_artifact_cache_key;

    fn record(name: &str) -> UnresolvedSourceRecord {
        let mut pr = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        UnresolvedSourceRecord {
            data: SourceRecordData::Full(FullSourceRecordData {
                package_record: pr,
                sources: BTreeMap::new(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: Utf8TypedPathBuf::from(format!("./{name}")),
            }),
            build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: None,
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        }
    }

    fn binary_dep(name: &str, url: &str, sha: &str) -> UnresolvedPixiRecord {
        let mut pr = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        pr.sha256 = parse_digest_from_hex::<rattler_digest::Sha256>(sha);
        let repo = RepoDataRecord {
            package_record: pr,
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                &format!("{name}-1.0.0-h0.conda"),
            )
            .unwrap(),
            url: url::Url::parse(url).unwrap(),
            channel: None,
        };
        UnresolvedPixiRecord::Binary(Arc::new(repo))
    }

    fn sha(byte: u8) -> Sha256Hash {
        let mut out = [0u8; 32];
        out[0] = byte;
        Sha256Hash::from(out)
    }

    fn key_for(
        r: &UnresolvedSourceRecord,
        backend_id: &str,
        extra_build_sha: &[Sha256Hash],
    ) -> String {
        compute_artifact_cache_key(
            r,
            Platform::Linux64,
            Platform::Linux64,
            backend_id,
            extra_build_sha,
            &[],
        )
        .to_string()
    }

    #[test]
    fn identical_inputs_produce_equal_keys() {
        let a = record("foo");
        let b = record("foo");
        assert_eq!(
            key_for(&a, "backend-v1", &[]),
            key_for(&b, "backend-v1", &[])
        );
    }

    #[test]
    fn package_name_matters() {
        let a = record("foo");
        let b = record("bar");
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn manifest_source_matters() {
        let a = record("foo");
        let mut b = record("foo");
        b.manifest_source = PinnedSourceSpec::Path(PinnedPathSpec {
            path: Utf8TypedPathBuf::from("./foo-alt"),
        });
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn variants_matter() {
        let a = record("foo");
        let mut b = record("foo");
        b.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.12".to_string()),
        );
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn variant_value_change_matters() {
        let mut a = record("foo");
        a.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.11".to_string()),
        );
        let mut b = record("foo");
        b.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.12".to_string()),
        );
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn build_platform_matters() {
        let r = record("foo");
        let k1 =
            compute_artifact_cache_key(&r, Platform::Linux64, Platform::Linux64, "b", &[], &[])
                .to_string();
        let k2 =
            compute_artifact_cache_key(&r, Platform::OsxArm64, Platform::Linux64, "b", &[], &[])
                .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn host_platform_matters() {
        let r = record("foo");
        let k1 =
            compute_artifact_cache_key(&r, Platform::Linux64, Platform::Linux64, "b", &[], &[])
                .to_string();
        let k2 =
            compute_artifact_cache_key(&r, Platform::Linux64, Platform::OsxArm64, "b", &[], &[])
                .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn backend_identifier_matters() {
        let r = record("foo");
        assert_ne!(
            key_for(&r, "backend-v1", &[]),
            key_for(&r, "backend-v2", &[])
        );
    }

    #[test]
    fn adding_a_binary_dep_changes_the_key() {
        let a = record("foo");
        let mut b = record("foo");
        b.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn binary_dep_url_matters() {
        let mut a = record("foo");
        a.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        let mut b = record("foo");
        b.build_packages.push(binary_dep(
            "numpy",
            "https://different.mirror/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn binary_dep_sha256_matters() {
        let mut a = record("foo");
        a.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        let mut b = record("foo");
        b.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "bb00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn build_source_dep_sha256_matters() {
        let r = record("foo");
        assert_ne!(
            key_for(&r, "b", &[sha(0xaa)]),
            key_for(&r, "b", &[sha(0xbb)]),
        );
    }

    #[test]
    fn host_source_dep_sha256_matters() {
        let r = record("foo");
        let k1 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xaa)],
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xbb)],
        )
        .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn source_dep_bucket_matters() {
        // A source dep with identical sha256 placed in the build vs host
        // bucket must hash to different keys. The bucket determines
        // which prefix it installs into and thus which compat map the
        // run-dep resolution sees.
        let r = record("foo");
        let build_only = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[sha(0xaa)],
            &[],
        )
        .to_string();
        let host_only = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xaa)],
        )
        .to_string();
        assert_ne!(build_only, host_only);
    }

    #[test]
    fn source_dep_order_within_a_bucket_matters() {
        // Two source deps with different sha256s in the same bucket:
        // swapping order produces a different key.
        let r = record("foo");
        assert_ne!(
            key_for(&r, "b", &[sha(0xaa), sha(0xbb)]),
            key_for(&r, "b", &[sha(0xbb), sha(0xaa)]),
        );
    }

    #[test]
    fn binary_dep_bucket_matters() {
        // Same binary dep placed in build_packages vs host_packages now
        // produces different keys (bucket-tagged). This is the fix for
        // the earlier collision bug.
        let mut a = record("foo");
        a.build_packages.push(binary_dep(
            "numpy",
            "https://x.test/numpy.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        let mut b = record("foo");
        b.host_packages.push(binary_dep(
            "numpy",
            "https://x.test/numpy.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn cache_key_is_deterministic_across_runs() {
        // Two independently-built records produce identical keys; the
        // hasher's state is reset per call.
        let r1 = record("foo");
        let r2 = record("foo");
        assert_eq!(
            key_for(&r1, "backend-v1", &[sha(0x01), sha(0x02)]),
            key_for(&r2, "backend-v1", &[sha(0x01), sha(0x02)]),
        );
    }

    #[test]
    fn cache_key_uses_host_platform_as_prefix() {
        let r = record("foo");
        let k =
            compute_artifact_cache_key(&r, Platform::Linux64, Platform::OsxArm64, "b", &[], &[])
                .to_string();
        assert!(k.starts_with("osx-arm64-"), "got: {k}");
    }
}
