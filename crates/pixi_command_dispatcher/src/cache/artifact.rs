//! Artifact cache for source builds.
//!
//! Layout (under the workspace's `.pixi/artifacts-v0/`, or the global
//! cache root when no workspace is set):
//!
//! ```text
//! <package_name>/<cache_key>/
//!     <pkg>-<ver>-<build>.conda
//!     sidecar.json
//! ```
//!
//! The cache key is structural (identity of the build inputs plus content
//! addresses of its dependencies). Freshness of the source files themselves
//! is tracked in the sidecar via per-file whole-second mtimes with a blake3
//! content-hash fallback, plus a re-glob pass that detects added files.
//!
//! On a hit, the cached `.conda` is returned along with its sha256; no
//! backend work happens. On a miss (or stale sidecar), the caller rebuilds
//! and calls [`ArtifactCache::store`] to populate the entry.
//!
//! Failed builds are never persisted; the compute engine caches failures in
//! memory for the lifetime of the process.

use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_fd_lock::{LockRead, LockWrite};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use pixi_build_types::InputGlobSet;
use pixi_compute_engine::ComputeCtx;
use pixi_manifest::InlineContentHash;
use pixi_path::{AbsPath, AbsPathBuf};
use pixi_record::{UnresolvedPixiRecord, UnresolvedSourceRecord};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

/// Opaque content-addressed handle for an artifact cache entry.
///
/// Format: url-safe-base64 of the `xxh3_64` over all hashed inputs
/// (see [`compute_artifact_cache_key`]). Package name is not included
/// because the entry lives under a `<package_name>/` parent directory.
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
/// - any user-supplied project-model overrides (build_string_prefix,
///   build_number) -- these flow into the resulting `.conda`'s build
///   string and number, so different overrides must not share a cache
///   entry
///
/// Source *files* are not hashed here: the sidecar captures their whole-second
/// mtimes and blake3 content hashes separately so a content change still
/// invalidates the entry on lookup.
///
/// The caller provides source-dep sha256s split into build / host buckets
/// to preserve the same bucket separation applied to binary deps.
#[allow(clippy::too_many_arguments)]
pub fn compute_artifact_cache_key(
    record: &UnresolvedSourceRecord,
    build_platform: Platform,
    host_platform: Platform,
    backend_identifier: &str,
    build_source_dep_sha256s: &[Sha256Hash],
    host_source_dep_sha256s: &[Sha256Hash],
    project_model_overrides: &crate::ProjectModelOverrides,
    package_format: Option<pixi_build_types::procedures::conda_build_v1::CondaPackageFormat>,
    inline_content_hash: Option<InlineContentHash>,
) -> ArtifactCacheKey {
    let mut hasher = Xxh3::new();
    record.name().as_normalized().hash(&mut hasher);
    record.manifest_source.hash(&mut hasher);
    record.build_source.hash(&mut hasher);
    record.variants.hash(&mut hasher);
    // An inline package definition's content hash is not otherwise
    // represented on disk, so it must enter the key explicitly: editing the
    // inline `[package]` table then invalidates the built artifact even when the
    // source files are untouched. `None` for ordinary source packages keeps
    // their key unchanged.
    inline_content_hash.hash(&mut hasher);
    build_platform.hash(&mut hasher);
    host_platform.hash(&mut hasher);
    backend_identifier.hash(&mut hasher);
    project_model_overrides.hash(&mut hasher);
    // Distinguish artifacts by output format.
    package_format.hash(&mut hasher);

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

    // `host_platform` is already folded into `hasher`, so the hash
    // alone uniquely identifies the artifact. Dropping the display-only
    // prefix keeps the on-disk path short on Windows.
    ArtifactCacheKey(URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes()))
}

/// On-disk record that lives next to a cached `.conda`.
///
/// Format has no version field by design: a parse failure triggers cache
/// invalidation, and fields are only ever added in a serde-backwards-compatible
/// way (new fields get `#[serde(default)]`).
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSidecar {
    /// Structured glob groups describing the files the build reads (the flat
    /// globs a backend reports are folded into a group upstream). Walked at
    /// lookup time to detect newly-added matching files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_glob_sets: Vec<InputGlobSet>,

    /// Files that matched the input globs, paired with the freshness
    /// fingerprint captured at build time. Keyed by absolute path (the walk
    /// roots are absolute), so the keys round-trip directly against the engine
    /// walk at lookup time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_files: BTreeMap<AbsPathBuf, InputFileFingerprint>,

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

/// Freshness fingerprint for a single input file, captured at build time.
///
/// `mtime` is truncated to whole-second precision because that is the coarsest
/// resolution that survives common transports: tar and Docker layer extraction,
/// zip archives, and FAT/exFAT all drop sub-second precision. Comparing at full
/// precision caused spurious rebuilds whenever a source tree round-tripped
/// through one of those (notably a Docker build sharing `PIXI_CACHE_DIR` via
/// `--mount=type=cache`, where `COPY` truncates mtimes to whole seconds).
///
/// `content` is a blake3 digest of the file bytes, used as the authoritative
/// fallback when the truncated mtime differs: an mtime that shifted without the
/// contents changing (again, routine after a Docker `COPY`) then still hits the
/// cache instead of forcing a rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputFileFingerprint {
    /// mtime of the file, truncated to whole-second precision.
    pub mtime: DateTime<Utc>,

    /// blake3 hash of the file contents, hex-encoded.
    pub content: String,
}

/// Truncate a timestamp to whole-second precision.
///
/// Sub-second precision does not survive tar/Docker/zip/FAT, so freshness
/// comparisons are done at the granularity that does.
fn truncate_to_secs(dt: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(dt.timestamp(), 0).unwrap_or(dt)
}

/// Compute the blake3 digest of a file's contents, hex-encoded.
pub(crate) fn blake3_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
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
        ctx: &mut ComputeCtx,
        package: &PackageName,
        key: &ArtifactCacheKey,
        source_dir: &AbsPath,
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

        // Check every recorded file is still fresh. The whole-second mtime is a
        // fast path; when it drifts we fall back to the content hash so an mtime
        // that merely round-tripped through a tar/Docker layer doesn't force a
        // rebuild.
        for (path, fingerprint) in &sidecar.input_files {
            let modified = match fs_err::metadata(path).and_then(|m| m.modified()) {
                Ok(m) => truncate_to_secs(DateTime::<Utc>::from(m)),
                Err(_) => return Ok(None),
            };
            // Fast path: the whole-second mtime is unchanged, trust it and skip
            // hashing the file.
            if modified == fingerprint.mtime {
                continue;
            }
            // The mtime drifted. That happens routinely when a source tree is
            // copied through a Docker layer (or any tar/zip) without the
            // contents changing, so compare the content hash before declaring
            // the entry stale.
            let content = match blake3_file(path.as_std_path()) {
                Ok(content) => content,
                Err(_) => return Ok(None),
            };
            if content != fingerprint.content {
                return Ok(None);
            }
        }

        // Detect newly-added files that match the stored globs. This catches
        // sources added after the cache entry was written. Uses the same
        // engine-deduped walk as `build_backend_metadata`.
        let current =
            crate::input_globs::collect_input_files(ctx, &sidecar.input_glob_sets, source_dir)
                .await
                .map_err(ArtifactCacheError::Glob)?;
        for matched in current {
            if !sidecar.input_files.contains_key(&matched) {
                return Ok(None);
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
    /// `input_files` are absolute paths; their mtimes are captured at store
    /// time. `record` is the synthesized `RepoDataRecord` for the artifact; it
    /// is persisted in the sidecar so cache hits can skip re-reading
    /// index.json.
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
        input_glob_sets: Vec<InputGlobSet>,
        input_files: impl IntoIterator<Item = AbsPathBuf>,
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

        let mut input_fingerprints = BTreeMap::new();
        for path in input_files {
            let modified = fs_err::metadata(&path)
                .and_then(|m| m.modified())
                .map_err(|err| {
                    ArtifactCacheError::io("stat input file", path.clone().into(), err)
                })?;
            let content = blake3_file(path.as_std_path()).map_err(|err| {
                ArtifactCacheError::io("hashing input file", path.clone().into(), err)
            })?;
            input_fingerprints.insert(
                path,
                InputFileFingerprint {
                    mtime: truncate_to_secs(DateTime::<Utc>::from(modified)),
                    content,
                },
            );
        }

        let sidecar = ArtifactSidecar {
            input_glob_sets,
            input_files: input_fingerprints,
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

    use filetime::FileTime;
    use pixi_compute_engine::ComputeEngine;
    use rattler_conda_types::{PackageName, PackageRecord};
    use tempfile::TempDir;

    use super::*;

    fn key(s: &str) -> ArtifactCacheKey {
        ArtifactCacheKey(s.to_string())
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::from_str(name).unwrap()
    }

    /// A single glob group with default (markers-free, hidden-excluding) config.
    fn glob_group(patterns: &[&str]) -> InputGlobSet {
        InputGlobSet {
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
            markers: Vec::new(),
            exclude_hidden: true,
            root: None,
        }
    }

    fn abs(path: impl Into<PathBuf>) -> AbsPathBuf {
        AbsPathBuf::new(path).unwrap()
    }

    /// Drive [`ArtifactCache::lookup`] (which needs a `ComputeCtx`) through the
    /// provided engine. The test source dirs are absolute tempdirs.
    async fn lookup(
        engine: &ComputeEngine,
        cache: &ArtifactCache,
        package: &PackageName,
        key: &ArtifactCacheKey,
        source: &Path,
    ) -> Result<Option<CachedArtifact>, ArtifactCacheError> {
        let source = AbsPath::new(source).unwrap();
        engine
            .with_ctx(async |ctx| cache.lookup(ctx, package, key, source).await)
            .await
            .expect("compute engine cycle")
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
        let engine = ComputeEngine::new();
        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let got = lookup(&engine, &cache, &pkg("foo"), &key("linux-64-abc"), &source)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn round_trip_store_then_lookup() {
        let tmp = TempDir::new().unwrap();
        let cache_root = tmp.path().join("artifacts");
        let cache = ArtifactCache::new(&cache_root);
        let engine = ComputeEngine::new();

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
                vec![glob_group(&["**/*.py"])],
                vec![abs(input.clone())],
                dummy_record("foo"),
            )
            .await
            .unwrap();

        let hit = lookup(&engine, &cache, &pkg("foo"), &key, &source)
            .await
            .unwrap();
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
        let engine = ComputeEngine::new();

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
                Vec::new(),
                vec![abs(input.clone())],
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Change the contents, and push the mtime to a clearly different
        // whole second so the fast-path mtime check registers the change
        // deterministically (a sub-second rewrite could land in the same
        // truncated second).
        fs_err::write(&input, b"new").unwrap();
        filetime::set_file_mtime(&input, FileTime::from_unix_time(2_000_000_000, 0)).unwrap();

        let got = lookup(&engine, &cache, &pkg("foo"), &key, &source)
            .await
            .unwrap();
        assert!(got.is_none(), "content change should invalidate the entry");
    }

    /// Regression for the Docker `--mount=type=cache` scenario: copying a
    /// source tree through an image layer rewrites mtimes (Docker truncates to
    /// whole seconds and may shift them) without changing file contents. The
    /// blake3 content-hash fallback must keep the entry valid instead of
    /// rebuilding the pixi-build package on every `pixi run`/`pixi install`.
    #[tokio::test]
    async fn lookup_hits_when_only_mtime_shifts() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let engine = ComputeEngine::new();

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let input = source.join("main.py");
        fs_err::write(&input, b"body").unwrap();

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
                Vec::new(),
                vec![abs(input.clone())],
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Rewrite the mtime to a different whole second but leave the bytes
        // untouched, mimicking a source tree copied through a Docker layer.
        filetime::set_file_mtime(&input, FileTime::from_unix_time(2_000_000_000, 0)).unwrap();

        let got = lookup(&engine, &cache, &pkg("foo"), &key, &source)
            .await
            .unwrap();
        assert!(
            got.is_some(),
            "an mtime shift with unchanged contents must still hit the cache"
        );
    }

    #[tokio::test]
    async fn lookup_stale_when_new_file_matches_glob() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let engine = ComputeEngine::new();

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let input = source.join("main.py");
        fs_err::write(&input, b"body").unwrap();

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
                vec![glob_group(&["**/*.py"])],
                vec![abs(input.clone())],
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Introduce a new file that matches the stored glob.
        fs_err::write(source.join("extra.py"), b"new module").unwrap();

        let got = lookup(&engine, &cache, &pkg("foo"), &key, &source)
            .await
            .unwrap();
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
        let engine = ComputeEngine::new();

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
                Vec::new(),
                Vec::<AbsPathBuf>::new(),
                record.clone(),
            )
            .await
            .unwrap();

        let hit = lookup(&engine, &cache, &pkg("foo"), &key, &source)
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
        let engine = ComputeEngine::new();
        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let input = source.join("main.py");
        fs_err::write(&input, b"body").unwrap();
        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"pretend").unwrap();

        let key = key("linux-64-abc");
        let pkg = pkg("foo");

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache = cache.clone();
            let input = input.clone();
            let artifact = artifact.clone();
            let pkg = pkg.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                cache
                    .store(
                        &pkg,
                        &key,
                        &artifact,
                        Vec::new(),
                        vec![abs(input)],
                        dummy_record("foo"),
                    )
                    .await
                    .unwrap();
            }));
        }
        for _ in 0..8 {
            let cache = cache.clone();
            let engine = engine.clone();
            let source = source.clone();
            let pkg = pkg.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                // Ignore whether it's a hit or miss; racing with stores
                // the lookup may see either. The contract under test is
                // that it never errors out.
                let _ = lookup(&engine, &cache, &pkg, &key, &source).await.unwrap();
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
        let engine = ComputeEngine::new();

        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();

        let entry = cache.entry_dir(&pkg("foo"), &key("linux-64-abc"));
        fs_err::create_dir_all(&entry).unwrap();
        fs_err::write(entry.join("sidecar.json"), b"{ this is not valid").unwrap();

        let got = lookup(&engine, &cache, &pkg("foo"), &key("linux-64-abc"), &source)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    /// Regression for prefix-dev/pixi#6232: a thin package whose manifest
    /// points at `../recipe.yaml` makes the build report a `../**` input
    /// glob. The walker rebases that onto the parent directory, so it matches
    /// files outside the package's own source dir (the recipe, the build
    /// script, sibling variant dirs). Those must round-trip through store +
    /// lookup so the second run is a cache hit instead of rebuilding from
    /// scratch.
    #[tokio::test]
    async fn issue_6232_parent_recipe_glob_does_not_force_rebuild() {
        let tmp = TempDir::new().unwrap();
        // Layout: <root>/llama-cpp/{recipe.yaml, build.sh, vulkan/{pixi.toml, variants.yaml}}
        let parent = tmp.path().join("llama-cpp");
        let source = parent.join("vulkan");
        fs_err::create_dir_all(&source).unwrap();
        fs_err::write(parent.join("recipe.yaml"), b"recipe").unwrap();
        fs_err::write(parent.join("build.sh"), b"script").unwrap();
        fs_err::write(source.join("pixi.toml"), b"manifest").unwrap();
        fs_err::write(source.join("variants.yaml"), b"backend:\n  - vulkan\n").unwrap();
        let source_abs = AbsPath::new(&source).unwrap();

        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let engine = ComputeEngine::new();
        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"artifact").unwrap();

        // The build reports `../**` because the recipe lives in the parent dir.
        let groups = vec![glob_group(&["../**"])];

        // Resolve the matched files exactly as the source-build pipeline does.
        let input_files = engine
            .with_ctx(async |ctx| {
                crate::input_globs::collect_input_files(ctx, &groups, source_abs).await
            })
            .await
            .unwrap()
            .unwrap();
        // Sanity: the `../**` glob really did reach the parent recipe.
        assert!(
            input_files
                .iter()
                .any(|p| p.as_std_path().ends_with("llama-cpp/recipe.yaml")),
            "expected `../**` to match the parent recipe, got {input_files:?}",
        );

        let key = key("linux-64-abc");
        cache
            .store(
                &pkg("foo"),
                &key,
                &artifact,
                groups,
                input_files,
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Second run with nothing changed: must hit, not rebuild.
        let hit = lookup(&engine, &cache, &pkg("foo"), &key, &source)
            .await
            .unwrap();
        assert!(
            hit.is_some(),
            "issue #6232: a `../`-recipe glob must not force a rebuild on the next run",
        );
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
            identifier_hash: String::new(),
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
            &Default::default(),
            None,
            None,
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
        let k1 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::OsxArm64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn host_platform_matters() {
        let r = record("foo");
        let k1 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::OsxArm64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
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
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xbb)],
            &Default::default(),
            None,
            None,
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
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let host_only = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xaa)],
            &Default::default(),
            None,
            None,
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
    fn cache_key_changes_with_host_platform() {
        // Host platform is hashed into the key but not displayed in
        // it (short-path policy). Two keys that differ only by host
        // platform must therefore be distinct.
        let r = record("foo");
        let linux = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        let osx_arm = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::OsxArm64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        assert_ne!(linux, osx_arm);
    }

    #[test]
    fn build_string_prefix_matters() {
        let r = record("foo");
        let bare = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        let prefixed = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &crate::ProjectModelOverrides {
                build_string_prefix: Some("foobar".to_string()),
                build_number: None,
            },
            None,
            None,
        );
        assert_ne!(bare, prefixed);
    }

    #[test]
    fn build_number_matters() {
        let r = record("foo");
        let bare = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        let numbered = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &crate::ProjectModelOverrides {
                build_string_prefix: None,
                build_number: Some(42),
            },
            None,
            None,
        );
        assert_ne!(bare, numbered);
    }

    #[test]
    fn archive_type_matters() {
        use pixi_build_types::procedures::conda_build_v1::CondaPackageFormat;
        use rattler_conda_types::package::CondaArchiveType;
        let r = record("foo");
        let conda = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            Some(CondaPackageFormat {
                archive_type: CondaArchiveType::Conda,
                compression_level: Default::default(),
            }),
            None,
        );
        let tar_bz2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            Some(CondaPackageFormat {
                archive_type: CondaArchiveType::TarBz2,
                compression_level: Default::default(),
            }),
            None,
        );
        assert_ne!(conda, tar_bz2);
    }

    #[test]
    fn compression_level_matters() {
        use pixi_build_types::procedures::conda_build_v1::{
            CondaCompressionLevel, CondaPackageFormat, NamedCompressionLevel,
        };
        use rattler_conda_types::package::CondaArchiveType;
        let pf = |level: CondaCompressionLevel| CondaPackageFormat {
            archive_type: CondaArchiveType::Conda,
            compression_level: level,
        };
        let r = record("foo");
        let key = |level: CondaCompressionLevel| {
            compute_artifact_cache_key(
                &r,
                Platform::Linux64,
                Platform::Linux64,
                "b",
                &[],
                &[],
                &Default::default(),
                Some(pf(level)),
                None,
            )
        };
        let default_level = key(CondaCompressionLevel::Named(NamedCompressionLevel::Default));
        let max_level = key(CondaCompressionLevel::Named(NamedCompressionLevel::Highest));
        let numeric_level = key(CondaCompressionLevel::Numeric(5));
        assert_ne!(default_level, max_level);
        assert_ne!(default_level, numeric_level);
        assert_ne!(max_level, numeric_level);
    }
}
