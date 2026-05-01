//! Extension traits on [`pixi_compute_engine::DataStore`] for ergonomic access
//! to pixi-specific shared resources from within
//! [`pixi_compute_engine::Key::compute`] bodies.
//!
//! Values are registered at engine construction time (via the
//! [`CommandDispatcherBuilder`](crate::CommandDispatcherBuilder) for production
//! use, or directly by tests that want to drive the engine without building a
//! full dispatcher). Each value is stored under its own `TypeId`, so a test
//! can populate only the entries its Keys read.

use std::sync::Arc;

use pixi_compute_engine::DataStore;
use pixi_git::resolver::GitResolver;
use pixi_url::UrlResolver;
use rattler::package_cache::PackageCache;
use rattler_networking::LazyClient;
use rattler_repodata_gateway::Gateway;
use tokio::sync::Semaphore;

use crate::{
    CacheDirs,
    cache::BuildBackendMetadataCache,
    reporter::Reporter,
};

/// Access the conda repodata gateway from global data.
pub trait HasGateway {
    fn gateway(&self) -> &Gateway;
}

impl HasGateway for DataStore {
    fn gateway(&self) -> &Gateway {
        self.get::<Gateway>()
    }
}

/// Access the git resolver from global data.
pub trait HasGitResolver {
    fn git_resolver(&self) -> &GitResolver;
}

impl HasGitResolver for DataStore {
    fn git_resolver(&self) -> &GitResolver {
        self.get::<GitResolver>()
    }
}

/// Access the URL resolver from global data.
pub trait HasUrlResolver {
    fn url_resolver(&self) -> &UrlResolver;
}

impl HasUrlResolver for DataStore {
    fn url_resolver(&self) -> &UrlResolver {
        self.get::<UrlResolver>()
    }
}

/// Access the download client from global data.
pub trait HasDownloadClient {
    fn download_client(&self) -> &LazyClient;
}

impl HasDownloadClient for DataStore {
    fn download_client(&self) -> &LazyClient {
        self.get::<LazyClient>()
    }
}

/// Access the cache directories from global data.
pub trait HasCacheDirs {
    fn cache_dirs(&self) -> &CacheDirs;
}

impl HasCacheDirs for DataStore {
    fn cache_dirs(&self) -> &CacheDirs {
        self.get::<CacheDirs>()
    }
}

/// Access the on-disk build-backend metadata cache from global data.
pub trait HasBuildBackendMetadataCache {
    fn build_backend_metadata_cache(&self) -> &BuildBackendMetadataCache;
}

impl HasBuildBackendMetadataCache for DataStore {
    fn build_backend_metadata_cache(&self) -> &BuildBackendMetadataCache {
        self.get::<BuildBackendMetadataCache>()
    }
}

/// Access the optional dispatcher reporter from global data.
pub trait HasReporter {
    fn reporter(&self) -> Option<&Arc<dyn Reporter>>;
}

impl HasReporter for DataStore {
    fn reporter(&self) -> Option<&Arc<dyn Reporter>> {
        self.try_get::<Arc<dyn Reporter>>()
    }
}

/// Access the package cache from global data.
pub trait HasPackageCache {
    fn package_cache(&self) -> &PackageCache;
}

impl HasPackageCache for DataStore {
    fn package_cache(&self) -> &PackageCache {
        self.get::<PackageCache>()
    }
}

/// Newtype around the `execute_link_scripts` bool so it can be stored
/// in [`DataStore`] keyed by its own `TypeId`.
#[derive(Copy, Clone, Debug)]
pub struct AllowExecuteLinkScripts(pub bool);

/// Access whether link-script execution is permitted.
pub trait HasAllowExecuteLinkScripts {
    fn allow_execute_link_scripts(&self) -> bool;
}

impl HasAllowExecuteLinkScripts for DataStore {
    fn allow_execute_link_scripts(&self) -> bool {
        self.try_get::<AllowExecuteLinkScripts>()
            .map(|v| v.0)
            .unwrap_or(false)
    }
}

/// Newtype around the semaphore that bounds concurrent conda solves.
/// Conda solves are CPU- and memory-intensive; this semaphore enforces
/// the `max_concurrent_solves` limit from [`crate::Limits`].
#[derive(Clone)]
pub struct CondaSolveSemaphore(pub Arc<Semaphore>);

/// Access the semaphore bounding concurrent conda solves. Returns
/// `None` when no semaphore was registered, treated as unbounded.
pub trait HasCondaSolveSemaphore {
    fn conda_solve_semaphore(&self) -> Option<&Arc<Semaphore>>;
}

impl HasCondaSolveSemaphore for DataStore {
    fn conda_solve_semaphore(&self) -> Option<&Arc<Semaphore>> {
        self.try_get::<CondaSolveSemaphore>().map(|s| &s.0)
    }
}

/// Newtype around the semaphore that bounds concurrent backend source
/// builds. Enforces the `max_concurrent_builds` limit from
/// [`crate::Limits`].
#[derive(Clone)]
pub struct BackendSourceBuildSemaphore(pub Arc<Semaphore>);

/// Access the semaphore bounding concurrent backend source builds. Returns
/// `None` when no semaphore was registered, treated as unbounded.
pub trait HasBackendSourceBuildSemaphore {
    fn backend_source_build_semaphore(&self) -> Option<&Arc<Semaphore>>;
}

impl HasBackendSourceBuildSemaphore for DataStore {
    fn backend_source_build_semaphore(&self) -> Option<&Arc<Semaphore>> {
        self.try_get::<BackendSourceBuildSemaphore>().map(|s| &s.0)
    }
}
