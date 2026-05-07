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
}

/// Cached git checkouts.
pub struct GitDir;
impl CacheLocation for GitDir {
    fn name() -> &'static str {
        consts::CACHED_GIT_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Root
    }
}

/// Cached URL archive checkouts.
pub struct UrlDir;
impl CacheLocation for UrlDir {
    fn name() -> &'static str {
        consts::CACHED_URL_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Root
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
