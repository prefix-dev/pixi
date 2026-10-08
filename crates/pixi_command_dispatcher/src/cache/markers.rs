//! `PackagesDir` declares an env-var override for
//! `PIXI_CACHE_CONDA_PACKAGES_DIR`.
//!
//! Each marker is a zero-sized type whose [`CacheLocation`] impl
//! describes a fixed `<base>/<name>` layout. Markers also key
//! programmatic overrides on
//! [`CacheDirs`](pixi_compute_cache_dirs::CacheDirs) via their
//! `TypeId`.

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
    use std::collections::HashMap;

    use pixi_compute_cache_dirs::CacheDirs;
    use pixi_path::AbsPathBuf;

    use super::*;

    #[test]
    fn test_packages_dir_default_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let root = AbsPathBuf::new(temp.path().to_path_buf())
            .unwrap()
            .into_assume_dir();
        let cache_dirs = CacheDirs::new(root.clone());
        let env = HashMap::new();
        let resolved = cache_dirs.resolve_with_env::<PackagesDir>(&env);
        assert_eq!(
            resolved,
            root.join(consts::CACHED_PACKAGES).into_assume_dir()
        );
    }

    #[test]
    fn test_packages_dir_env_override() {
        let temp_root = tempfile::tempdir().unwrap();
        let root = AbsPathBuf::new(temp_root.path().to_path_buf())
            .unwrap()
            .into_assume_dir();
        let temp_pkgs = tempfile::tempdir().unwrap();
        let custom_pkgs = AbsPathBuf::new(temp_pkgs.path().to_path_buf())
            .unwrap()
            .into_assume_dir();
        let cache_dirs = CacheDirs::new(root);
        let mut env = HashMap::new();
        env.insert(
            "PIXI_CACHE_CONDA_PACKAGES_DIR".to_string(),
            custom_pkgs.as_std_path().to_string_lossy().to_string(),
        );
        let resolved = cache_dirs.resolve_with_env::<PackagesDir>(&env);
        assert_eq!(resolved, custom_pkgs);
    }
}
