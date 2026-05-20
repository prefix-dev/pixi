//! Workspace cache for source builds.
//!
//! Layout (under the workspace's `.pixi/bld/`, or the global cache
//! root when no workspace is set). The short directory name is
//! deliberate: backend build trees are deeply nested and the full
//! path must stay under Windows' `MAX_PATH = 260`.
//!
//! ```text
//! <package_name>/<workspace_key>/
//!     ... (backend-managed build tree: prefixes, ninja/cmake state, etc.)
//! ```
//!
//! Unlike the artifact cache, this cache is keyed on the **full dep set**
//! (Cargo-style): any change to `build_packages` or `host_packages` produces
//! a new workspace key. This is correctness-driven: CMake/autotools builds
//! bake fixed prefix paths into their own incremental state, so a dep swap
//! under the same prefix is invisible to the build tool and must be treated
//! as a fresh workspace.
//!
//! The cost is that a dep update blows away incremental state for packages
//! downstream of it. The mitigation is the artifact cache: when the same
//! (source, deps, variants) combination is seen again, the rebuild hits the
//! artifact cache and skips the workspace entirely.

use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
};

use async_fd_lock::{LockWrite, RwLockWriteGuard};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pixi_record::UnresolvedSourceRecord;
use rattler_conda_types::{PackageName, Platform};
use xxhash_rust::xxh3::Xxh3;

/// Opaque handle identifying one workspace cache entry.
///
/// Format: url-safe-base64 of the `xxh3_64` over all hashed inputs
/// (see [`compute_workspace_key`]). Scoped under a `<package_name>/`
/// parent dir so per-package nuking is a single directory remove.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceKey(String);

impl std::fmt::Display for WorkspaceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Compute the workspace cache key for a source build.
///
/// Inputs that go into the hash:
/// - package name, pinned manifest source, pinned build source, variants
/// - build + host platform
/// - backend identifier
/// - the full `build_packages` and `host_packages` lists (not content-
///   addressed like the artifact cache; structural identity is what matters
///   here, so the workspace is stable across runs that produce identical
///   dep sets)
pub fn compute_workspace_key(
    record: &UnresolvedSourceRecord,
    build_platform: Platform,
    host_platform: Platform,
    backend_identifier: &str,
) -> WorkspaceKey {
    let mut hasher = Xxh3::new();
    record.name().as_normalized().hash(&mut hasher);
    record.manifest_source.hash(&mut hasher);
    record.build_source.hash(&mut hasher);
    record.variants.hash(&mut hasher);
    build_platform.hash(&mut hasher);
    host_platform.hash(&mut hasher);
    backend_identifier.hash(&mut hasher);
    record.build_packages.hash(&mut hasher);
    record.host_packages.hash(&mut hasher);
    // `host_platform` is already folded into `hasher`, so the key
    // hash alone uniquely identifies the workspace. No display-only
    // prefix: keeps the on-disk path short on Windows.
    WorkspaceKey(URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes()))
}

/// Workspace cache rooted at `<cache_root>/workspaces/`.
#[derive(Clone, Debug)]
pub struct WorkspaceCache {
    root: PathBuf,
}

impl WorkspaceCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Directory that holds every workspace for `package`.
    pub fn package_dir(&self, package: &PackageName) -> PathBuf {
        self.root.join(package.as_normalized())
    }

    /// Directory that holds the workspace for `(package, key)`. Existence
    /// is not guaranteed; call [`ensure_dir`](Self::ensure_dir) to create it.
    pub fn dir(&self, package: &PackageName, key: &WorkspaceKey) -> PathBuf {
        self.package_dir(package).join(&key.0)
    }

    /// Create the workspace directory if it does not exist and return it.
    pub fn ensure_dir(
        &self,
        package: &PackageName,
        key: &WorkspaceKey,
    ) -> std::io::Result<PathBuf> {
        let path = self.dir(package, key);
        fs_err::create_dir_all(&path)?;
        Ok(path)
    }

    /// Create the workspace directory and take an exclusive
    /// cross-process lock on it. The returned guard owns the lock;
    /// drop it after the backend build finishes.
    ///
    /// The lock serializes backend invocations for the same
    /// `(package, key)` across processes, so CMake/autotools-style
    /// backends that mutate the workspace in-place don't corrupt each
    /// other. Other processes calling this method for the same
    /// workspace block until the current holder drops the guard.
    pub async fn ensure_dir_locked(
        &self,
        package: &PackageName,
        key: &WorkspaceKey,
    ) -> std::io::Result<WorkspaceGuard> {
        let path = self.dir(package, key);
        tokio::fs::create_dir_all(&path).await?;
        let lock_path = path.join(".lock");
        let lock_file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .await?;
        let guard = lock_file.lock_write().await.map_err(|e| e.error)?;
        Ok(WorkspaceGuard {
            path,
            _guard: guard,
        })
    }

    /// Remove every workspace (and its backend-managed state) for
    /// `package`. Silently succeeds if nothing is cached. Intended for
    /// CLI invalidation commands like `pixi build --clean`.
    pub fn clear_package(&self, package: &PackageName) -> std::io::Result<()> {
        let dir = self.package_dir(package);
        match fs_err::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Remove every workspace across every package.
    pub fn clear_all(&self) -> std::io::Result<()> {
        match fs_err::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

/// Exclusive-locked handle to a workspace directory. Returned by
/// [`WorkspaceCache::ensure_dir_locked`]; releases the lock on drop.
///
/// Callers pass [`Self::path`] to the backend build. Other processes
/// calling `ensure_dir_locked` for the same workspace block until this
/// guard is dropped.
pub struct WorkspaceGuard {
    path: PathBuf,
    _guard: RwLockWriteGuard<tokio::fs::File>,
}

impl WorkspaceGuard {
    /// Absolute path to the workspace directory.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, str::FromStr, sync::Arc};

    use pixi_record::{
        FullSourceRecordData, PinnedPathSpec, PinnedSourceSpec, SourceRecordData,
        UnresolvedPixiRecord, UnresolvedSourceRecord,
    };
    use rattler_conda_types::{PackageName, PackageRecord};
    use tempfile::TempDir;
    use typed_path::Utf8TypedPathBuf;

    use super::*;

    fn make_record(name: &str) -> UnresolvedSourceRecord {
        let mut pkg = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pkg.subdir = "linux-64".into();
        UnresolvedSourceRecord {
            data: SourceRecordData::Full(FullSourceRecordData {
                package_record: pkg,
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

    fn with_build_dep(
        mut record: UnresolvedSourceRecord,
        dep: UnresolvedPixiRecord,
    ) -> UnresolvedSourceRecord {
        record.build_packages.push(dep);
        record
    }

    fn binary_dep(name: &str, url: &str) -> UnresolvedPixiRecord {
        let mut pkg = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pkg.subdir = "linux-64".into();
        let record = rattler_conda_types::RepoDataRecord {
            package_record: pkg,
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                "foo-1.0.0-h0.conda",
            )
            .unwrap(),
            url: url::Url::parse(url).unwrap(),
            channel: None,
        };
        UnresolvedPixiRecord::Binary(Arc::new(record))
    }

    #[test]
    fn same_inputs_produce_same_key() {
        let a = make_record("foo");
        let b = make_record("foo");
        let ka = compute_workspace_key(&a, Platform::Linux64, Platform::Linux64, "backend-v1");
        let kb = compute_workspace_key(&b, Platform::Linux64, Platform::Linux64, "backend-v1");
        assert_eq!(ka, kb);
    }

    #[test]
    fn different_deps_produce_different_keys() {
        let base = make_record("foo");
        let with_dep = with_build_dep(
            make_record("foo"),
            binary_dep("bar", "https://example.com/bar.conda"),
        );
        let ka = compute_workspace_key(&base, Platform::Linux64, Platform::Linux64, "backend-v1");
        let kb = compute_workspace_key(
            &with_dep,
            Platform::Linux64,
            Platform::Linux64,
            "backend-v1",
        );
        assert_ne!(ka, kb);
    }

    #[test]
    fn different_backend_identifier_changes_key() {
        let r = make_record("foo");
        let ka = compute_workspace_key(&r, Platform::Linux64, Platform::Linux64, "backend-v1");
        let kb = compute_workspace_key(&r, Platform::Linux64, Platform::Linux64, "backend-v2");
        assert_ne!(ka, kb);
    }

    #[test]
    fn key_changes_with_host_platform() {
        // Host platform is folded into the hash but not displayed in
        // the key (short-path policy). Two keys that differ only by
        // host platform must therefore be distinct.
        let r = make_record("foo");
        let linux = compute_workspace_key(&r, Platform::Linux64, Platform::Linux64, "backend-v1");
        let osx_arm =
            compute_workspace_key(&r, Platform::Linux64, Platform::OsxArm64, "backend-v1");
        assert_ne!(linux, osx_arm);
    }

    #[test]
    fn package_name_changes_key() {
        let a = make_record("foo");
        let b = make_record("bar");
        assert_ne!(
            compute_workspace_key(&a, Platform::Linux64, Platform::Linux64, "b"),
            compute_workspace_key(&b, Platform::Linux64, Platform::Linux64, "b"),
        );
    }

    #[test]
    fn manifest_source_changes_key() {
        let a = make_record("foo");
        let mut b = make_record("foo");
        b.manifest_source = PinnedSourceSpec::Path(PinnedPathSpec {
            path: Utf8TypedPathBuf::from("./somewhere-else"),
        });
        assert_ne!(
            compute_workspace_key(&a, Platform::Linux64, Platform::Linux64, "b"),
            compute_workspace_key(&b, Platform::Linux64, Platform::Linux64, "b"),
        );
    }

    #[test]
    fn variants_change_key() {
        let a = make_record("foo");
        let mut b = make_record("foo");
        b.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.12".to_string()),
        );
        assert_ne!(
            compute_workspace_key(&a, Platform::Linux64, Platform::Linux64, "b"),
            compute_workspace_key(&b, Platform::Linux64, Platform::Linux64, "b"),
        );
    }

    #[test]
    fn build_platform_changes_key() {
        let r = make_record("foo");
        assert_ne!(
            compute_workspace_key(&r, Platform::Linux64, Platform::Linux64, "b"),
            compute_workspace_key(&r, Platform::OsxArm64, Platform::Linux64, "b"),
        );
    }

    #[test]
    fn host_packages_matter_too() {
        // The workspace key is Cargo-style: any change to the full dep set
        // (build OR host) produces a fresh workspace. Verify that a host-
        // only dep change also busts the key.
        let base = make_record("foo");
        let mut with_host_dep = make_record("foo");
        with_host_dep
            .host_packages
            .push(binary_dep("zlib", "https://example.com/zlib.conda"));
        assert_ne!(
            compute_workspace_key(&base, Platform::Linux64, Platform::Linux64, "b"),
            compute_workspace_key(&with_host_dep, Platform::Linux64, Platform::Linux64, "b"),
        );
    }

    #[test]
    fn dep_order_within_a_bucket_matters() {
        // Two deps in build_packages; swapping their order yields a
        // different workspace key. The orchestrator must therefore feed
        // deps in a stable order.
        let mut a = make_record("foo");
        a.build_packages
            .push(binary_dep("numpy", "https://example.com/numpy.conda"));
        a.build_packages
            .push(binary_dep("zlib", "https://example.com/zlib.conda"));

        let mut b = make_record("foo");
        b.build_packages
            .push(binary_dep("zlib", "https://example.com/zlib.conda"));
        b.build_packages
            .push(binary_dep("numpy", "https://example.com/numpy.conda"));

        assert_ne!(
            compute_workspace_key(&a, Platform::Linux64, Platform::Linux64, "b"),
            compute_workspace_key(&b, Platform::Linux64, Platform::Linux64, "b"),
        );
    }

    #[test]
    fn ensure_dir_creates_layout() {
        let tmp = TempDir::new().unwrap();
        let cache = WorkspaceCache::new(tmp.path().join("workspaces"));
        let pkg = PackageName::from_str("foo").unwrap();
        let key = WorkspaceKey("linux-64-abc".into());
        let dir = cache.ensure_dir(&pkg, &key).unwrap();
        assert!(dir.is_dir());
        assert!(
            dir.ends_with("workspaces/foo/linux-64-abc")
                || dir.ends_with("workspaces\\foo\\linux-64-abc")
        );
    }

    /// Smoke test for the cross-process lock: two concurrent
    /// `ensure_dir_locked` calls for the same (pkg, key) serialize.
    /// The second waits until the first drops its guard.
    #[tokio::test]
    async fn ensure_dir_locked_serializes_concurrent_callers() {
        let tmp = TempDir::new().unwrap();
        let cache = WorkspaceCache::new(tmp.path().join("workspaces"));
        let pkg = PackageName::from_str("foo").unwrap();
        let key = WorkspaceKey("linux-64-abc".into());

        let first = cache.ensure_dir_locked(&pkg, &key).await.unwrap();

        // Spawn a second task that will block on the lock. Verify it
        // hasn't completed within a brief window.
        let second_task = {
            let cache = cache.clone();
            let pkg = pkg.clone();
            let key = key.clone();
            tokio::spawn(async move { cache.ensure_dir_locked(&pkg, &key).await.unwrap() })
        };
        let pending = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await
        })
        .await;
        assert!(pending.is_err(), "timeout guard should fire");
        assert!(
            !second_task.is_finished(),
            "second ensure_dir_locked should still be blocked while the first guard is held"
        );

        // Drop the first guard and the second acquisition should
        // complete promptly.
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(5), second_task)
            .await
            .expect("second lock acquisition should complete once the first is released")
            .expect("spawned task should not panic");
    }
}
