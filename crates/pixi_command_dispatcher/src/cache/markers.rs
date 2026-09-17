//! [`CacheLocation`] markers for every cache directory the dispatcher
//! knows about.
//!
//! Each marker is a zero-sized type whose [`CacheLocation`] impl
//! describes a fixed `<base>/<name>` layout. Markers also key
//! programmatic overrides on
//! [`CacheDirs`](pixi_compute_cache_dirs::CacheDirs) via their
//! `TypeId`. None declare an env-var override yet; behaviour is
//! identical to the previous synchronous getters until a marker opts
//! in.

use pixi_compute_cache_dirs::{CacheBase, CacheLocation};
use pixi_consts::consts;

/// Cached ephemeral build-backend prefixes.
pub struct BuildBackendsDir;
impl CacheLocation for BuildBackendsDir {
    fn name() -> &'static str {
        consts::CACHED_BUILD_BACKENDS
    }
    fn base() -> CacheBase {
        CacheBase::Root
    }
}

/// Binary package cache (rattler `PackageCache`).
pub struct PackagesDir;
impl CacheLocation for PackagesDir {
    fn name() -> &'static str {
        consts::CACHED_PACKAGES
    }
    fn base() -> CacheBase {
        CacheBase::Root
    }
    fn env_override() -> Option<&'static str> {
        Some("PIXI_CACHE_CONDA_PACKAGES_DIR")
    }
}

/// Backend metadata cache + per-source backend scratch tree.
///
/// The `meta-v0` literal mirrors `consts::CACHED_BUILD_BACKEND_METADATA`
/// concatenated with the cache impl's `CACHE_SUFFIX`. Bump both when
/// the on-disk layout changes incompatibly.
pub struct BackendMetadataDir;
impl CacheLocation for BackendMetadataDir {
    fn name() -> &'static str {
        "meta-v0"
    }
    fn base() -> CacheBase {
        CacheBase::Workspace
    }
}

/// Content-addressed source-build artifact cache.
pub struct SourceBuildArtifactsDir;
impl CacheLocation for SourceBuildArtifactsDir {
    fn name() -> &'static str {
        consts::SOURCE_BUILD_ARTIFACTS_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Workspace
    }
}

/// Per-package backend workspace tree (incremental backend state).
pub struct SourceBuildWorkspacesDir;
impl CacheLocation for SourceBuildWorkspacesDir {
    fn name() -> &'static str {
        consts::SOURCE_BUILD_WORKSPACES_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Workspace
    }
}

/// Cached pre-v7 source build/host environments (legacy
/// satisfiability path in `pixi_core`).
pub struct LegacySourceEnvDir;
impl CacheLocation for LegacySourceEnvDir {
    fn name() -> &'static str {
        consts::LEGACY_SOURCE_ENV_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Workspace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_compute_cache_dirs::CacheDirs;
    use pixi_path::AbsPathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_packages_dir_env_override() {
        let root = tempdir().unwrap();
        let root_abs = AbsPathBuf::new(root.path().to_path_buf())
            .unwrap()
            .into_assume_dir();
        let cache_dirs = CacheDirs::new(root_abs);

        let custom_dir = tempdir().unwrap();
        let custom_abs = custom_dir.path().to_string_lossy().to_string();

        let resolved = cache_dirs.resolve::<PackagesDir>(|var| {
            if var == "PIXI_CACHE_CONDA_PACKAGES_DIR" {
                Some(custom_abs.clone())
            } else {
                None
            }
        });

        assert_eq!(resolved.as_std_path(), custom_dir.path());
    }

    #[test]
    fn test_packages_dir_programmatic_override_wins() {
        let root = tempdir().unwrap();
        let root_abs = AbsPathBuf::new(root.path().to_path_buf())
            .unwrap()
            .into_assume_dir();
        let mut cache_dirs = CacheDirs::new(root_abs);

        let override_dir = tempdir().unwrap();
        let override_abs = AbsPathBuf::new(override_dir.path().to_path_buf())
            .unwrap()
            .into_assume_dir();
        cache_dirs.set_override::<PackagesDir>(override_abs);

        let custom_dir = tempdir().unwrap();
        let custom_abs = custom_dir.path().to_string_lossy().to_string();

        let resolved = cache_dirs.resolve::<PackagesDir>(|var| {
            if var == "PIXI_CACHE_CONDA_PACKAGES_DIR" {
                Some(custom_abs.clone())
            } else {
                None
            }
        });

        assert_eq!(resolved.as_std_path(), override_dir.path());
    }
}
