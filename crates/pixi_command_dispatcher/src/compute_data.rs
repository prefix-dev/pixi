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

use crate::cache::{BuildBackendMetadataCache, CacheDirs};
use crate::reporter::{
    BackendSourceBuildReporter, BuildBackendMetadataReporter, CondaSolveReporter,
    GitCheckoutReporter, InstantiateBackendReporter, PixiInstallReporter, PixiSolveReporter,
    SourceMetadataReporter, SourceRecordReporter, UrlCheckoutReporter,
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

/// Access the per-key git-checkout reporter.
pub trait HasGitCheckoutReporter {
    fn git_checkout_reporter(&self) -> Option<&Arc<dyn GitCheckoutReporter>>;
}

impl HasGitCheckoutReporter for DataStore {
    fn git_checkout_reporter(&self) -> Option<&Arc<dyn GitCheckoutReporter>> {
        self.try_get::<Arc<dyn GitCheckoutReporter>>()
    }
}

/// Access the per-key url-checkout reporter.
pub trait HasUrlCheckoutReporter {
    fn url_checkout_reporter(&self) -> Option<&Arc<dyn UrlCheckoutReporter>>;
}

impl HasUrlCheckoutReporter for DataStore {
    fn url_checkout_reporter(&self) -> Option<&Arc<dyn UrlCheckoutReporter>> {
        self.try_get::<Arc<dyn UrlCheckoutReporter>>()
    }
}

/// Access the per-key conda-solve reporter.
pub trait HasCondaSolveReporter {
    fn conda_solve_reporter(&self) -> Option<&Arc<dyn CondaSolveReporter>>;
}

impl HasCondaSolveReporter for DataStore {
    fn conda_solve_reporter(&self) -> Option<&Arc<dyn CondaSolveReporter>> {
        self.try_get::<Arc<dyn CondaSolveReporter>>()
    }
}

/// Access the per-key pixi-solve reporter.
pub trait HasPixiSolveReporter {
    fn pixi_solve_reporter(&self) -> Option<&Arc<dyn PixiSolveReporter>>;
}

impl HasPixiSolveReporter for DataStore {
    fn pixi_solve_reporter(&self) -> Option<&Arc<dyn PixiSolveReporter>> {
        self.try_get::<Arc<dyn PixiSolveReporter>>()
    }
}

/// Access the per-key pixi-install reporter.
pub trait HasPixiInstallReporter {
    fn pixi_install_reporter(&self) -> Option<&Arc<dyn PixiInstallReporter>>;
}

impl HasPixiInstallReporter for DataStore {
    fn pixi_install_reporter(&self) -> Option<&Arc<dyn PixiInstallReporter>> {
        self.try_get::<Arc<dyn PixiInstallReporter>>()
    }
}

/// Access the per-key instantiate-backend reporter.
pub trait HasInstantiateBackendReporter {
    fn instantiate_backend_reporter(&self) -> Option<&Arc<dyn InstantiateBackendReporter>>;
}

impl HasInstantiateBackendReporter for DataStore {
    fn instantiate_backend_reporter(&self) -> Option<&Arc<dyn InstantiateBackendReporter>> {
        self.try_get::<Arc<dyn InstantiateBackendReporter>>()
    }
}

/// Access the per-key build-backend-metadata reporter.
pub trait HasBuildBackendMetadataReporter {
    fn build_backend_metadata_reporter(&self) -> Option<&Arc<dyn BuildBackendMetadataReporter>>;
}

impl HasBuildBackendMetadataReporter for DataStore {
    fn build_backend_metadata_reporter(&self) -> Option<&Arc<dyn BuildBackendMetadataReporter>> {
        self.try_get::<Arc<dyn BuildBackendMetadataReporter>>()
    }
}

/// Access the per-key source-metadata reporter.
pub trait HasSourceMetadataReporter {
    fn source_metadata_reporter(&self) -> Option<&Arc<dyn SourceMetadataReporter>>;
}

impl HasSourceMetadataReporter for DataStore {
    fn source_metadata_reporter(&self) -> Option<&Arc<dyn SourceMetadataReporter>> {
        self.try_get::<Arc<dyn SourceMetadataReporter>>()
    }
}

/// Access the per-key source-record reporter.
pub trait HasSourceRecordReporter {
    fn source_record_reporter(&self) -> Option<&Arc<dyn SourceRecordReporter>>;
}

impl HasSourceRecordReporter for DataStore {
    fn source_record_reporter(&self) -> Option<&Arc<dyn SourceRecordReporter>> {
        self.try_get::<Arc<dyn SourceRecordReporter>>()
    }
}

/// Access the per-key backend-source-build reporter.
pub trait HasBackendSourceBuildReporter {
    fn backend_source_build_reporter(&self) -> Option<&Arc<dyn BackendSourceBuildReporter>>;
}

impl HasBackendSourceBuildReporter for DataStore {
    fn backend_source_build_reporter(&self) -> Option<&Arc<dyn BackendSourceBuildReporter>> {
        self.try_get::<Arc<dyn BackendSourceBuildReporter>>()
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

/// Configured allow/disallow preferences for installation link methods,
/// stored in the [`DataStore`] keyed by `TypeId`. Mirrors the fields on
/// [`rattler::install::LinkOptions`] but is `Copy`/`Clone`/`Debug` so it
/// can live in the data store.
#[derive(Copy, Clone, Debug, Default)]
pub struct AllowLinkOptions {
    pub allow_symbolic_links: Option<bool>,
    pub allow_hard_links: Option<bool>,
    pub allow_ref_links: Option<bool>,
}

impl From<AllowLinkOptions> for rattler::install::LinkOptions {
    fn from(opts: AllowLinkOptions) -> Self {
        rattler::install::LinkOptions {
            allow_symbolic_links: opts.allow_symbolic_links,
            allow_hard_links: opts.allow_hard_links,
            allow_ref_links: opts.allow_ref_links,
        }
    }
}

/// Access the configured link-method preferences.
pub trait HasAllowLinkOptions {
    fn allow_link_options(&self) -> rattler::install::LinkOptions;
}

impl HasAllowLinkOptions for DataStore {
    fn allow_link_options(&self) -> rattler::install::LinkOptions {
        self.try_get::<AllowLinkOptions>()
            .copied()
            .unwrap_or_default()
            .into()
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
