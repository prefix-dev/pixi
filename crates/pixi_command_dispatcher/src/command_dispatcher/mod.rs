//! Defines the [`CommandDispatcher`] and its associated components.
//!
//! [`CommandDispatcher`] is a thin, cheaply cloneable handle that wraps a
//! [`ComputeEngine`] together with shared
//! [`CommandDispatcherData`] (gateway, caches, resolvers, executor, etc.).
//! Public methods on the handle build the appropriate
//! [`Key`](pixi_compute_engine::Key) and submit it to the engine, which
//! handles deduplication, caching, and dependency tracking for operations
//! such as solving environments, fetching metadata, and managing source
//! checkouts.

use std::sync::Arc;

pub use builder::{CommandDispatcherBuilder, ReporterContextSpawnHook};
pub use error::{CommandDispatcherError, CommandDispatcherErrorResultExt, ComputeResultExt};
use pixi_build_frontend::BackendOverride;
use pixi_compute_engine::ComputeEngine;
use pixi_git::resolver::GitResolver;
use pixi_glob::GlobHashCache;
use pixi_url::UrlResolver;
use rattler::package_cache::PackageCache;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_networking::LazyClient;
use rattler_repodata_gateway::Gateway;
use tokio::sync::Semaphore;

use crate::{
    BackendHandle, BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataSpec,
    DevSourceMetadata, DevSourceMetadataError, DevSourceMetadataSpec, Executor,
    InstantiateBackendError, InstantiateBackendKey, Reporter,
    cache::{BuildBackendMetadataCache, CacheDirs},
    environment::WorkspaceEnvRegistry,
    install_pixi::{
        InstallPixiEnvironmentError, InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec,
    },
    instantiate_tool_env::{
        InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult,
        InstantiateToolEnvironmentSpec,
    },
};

mod builder;
mod error;

/// The command dispatcher is responsible for synchronizing requests between
/// different conda environments.
#[derive(Clone)]
pub struct CommandDispatcher {
    /// Holds the shared data required by the command dispatcher.
    pub(crate) data: Arc<CommandDispatcherData>,

    /// The generic compute engine. All real work runs through Keys
    /// computed via this engine.
    pub(crate) engine: ComputeEngine,

    /// The progress reporter. Shared among clones so `clear_reporter`
    /// can call through to it without routing via a background task.
    pub(crate) reporter: Option<Arc<dyn Reporter>>,

    /// Held so that when the last [`CommandDispatcher`] clone drops,
    /// the compute dep-graph snapshot is written if the
    /// `PIXI_COMPUTE_DEP_GRAPH` env var is set.
    _dump_guard: Arc<DepGraphDumpGuard>,
}

/// Dumps the compute-engine dependency graph when dropped if
/// `PIXI_COMPUTE_DEP_GRAPH` is set. Held in an `Arc` so the dump runs
/// only when the last [`CommandDispatcher`] clone is dropped, after
/// which the graph reflects the full session.
pub(crate) struct DepGraphDumpGuard {
    pub engine: ComputeEngine,
}

impl Drop for DepGraphDumpGuard {
    fn drop(&mut self) {
        let Ok(path) = std::env::var("PIXI_COMPUTE_DEP_GRAPH") else {
            return;
        };
        let graph = pixi_compute_engine::DependencyGraph::from_engine(&self.engine);
        match graph.write_dot(&path) {
            Ok(()) => tracing::info!("wrote compute-engine dependency graph to `{path}`"),
            Err(err) => {
                tracing::warn!("failed to write PIXI_COMPUTE_DEP_GRAPH dot file to `{path}`: {err}")
            }
        }
    }
}

/// Contains shared data required by the [`CommandDispatcher`].
///
/// This struct holds various components such as the gateway for querying
/// repodata, cache directories, and network clients.
pub(crate) struct CommandDispatcherData {
    /// The gateway to use to query conda repodata.
    pub gateway: Gateway,

    /// Backend metadata cache used to store metadata for source packages.
    pub build_backend_metadata_cache: BuildBackendMetadataCache,

    /// The resolver of git repositories.
    pub git_resolver: GitResolver,

    /// The resolver of url archives.
    pub url_resolver: UrlResolver,

    /// The location to store caches.
    pub cache_dirs: CacheDirs,

    /// The reqwest client to use for network requests.
    pub download_client: LazyClient,

    /// Backend overrides for build environments.
    pub build_backend_overrides: BackendOverride,

    /// A cache for glob hashes.
    pub glob_hash_cache: GlobHashCache,

    /// The package cache used to store packages.
    pub package_cache: PackageCache,

    /// The platform (and virtual packages) to use for tools that should run on
    /// the current system. Usually this is the current platform, but it can
    /// be a different platform.
    pub tool_platform: (Platform, Vec<GenericVirtualPackage>),

    /// True if execution of link scripts is enabled.
    pub execute_link_scripts: bool,

    /// The execution type of the dispatcher.
    pub executor: Executor,

    /// Semaphore that bounds concurrent git checkouts driven through
    /// the compute engine. `None` means unbounded.
    pub git_checkout_semaphore: Option<Arc<Semaphore>>,

    /// Semaphore that bounds concurrent URL archive fetches driven
    /// through the compute engine. `None` means unbounded.
    pub url_checkout_semaphore: Option<Arc<Semaphore>>,

    /// Semaphore that bounds concurrent conda solves driven through
    /// the compute engine. `None` means unbounded.
    pub conda_solve_semaphore: Option<Arc<Semaphore>>,

    /// Semaphore that bounds concurrent backend source builds driven
    /// through the compute engine. `None` means unbounded.
    pub backend_source_build_semaphore: Option<Arc<Semaphore>>,

    /// Registry of workspace environment specs reachable by id. Callers
    /// allocate refs via [`CommandDispatcher::workspace_env_registry`]
    /// and pass them into Keys that carry an
    /// [`EnvironmentRef`](crate::EnvironmentRef). A clone of this Arc
    /// is also registered in the compute engine's `DataStore` so
    /// projection compute bodies can resolve refs via
    /// `ctx.global_data().workspace_env_registry().get(id)`.
    pub workspace_env_registry: Arc<WorkspaceEnvRegistry>,
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandDispatcher {
    /// Constructs a new default constructed instance.
    pub fn new() -> Self {
        Self::builder().finish()
    }

    /// Constructs a new builder for the command dispatcher.
    pub fn builder() -> CommandDispatcherBuilder {
        CommandDispatcherBuilder::default()
    }

    /// Returns the executor used by the command dispatcher.
    pub fn executor(&self) -> Executor {
        self.data.executor
    }

    /// Returns a reference to the compute engine. The engine handles
    /// Key-based computations with automatic deduplication, caching,
    /// and cycle detection.
    pub fn engine(&self) -> &ComputeEngine {
        &self.engine
    }

    /// Returns the cache for source metadata.
    pub fn build_backend_metadata_cache(&self) -> &BuildBackendMetadataCache {
        &self.data.build_backend_metadata_cache
    }

    /// Returns the source-build artifact cache rooted at
    /// `cache_dirs.source_build_artifacts()`. Use this to invalidate
    /// cached build outputs for a package (e.g. when implementing
    /// `--force-reinstall` or `--clean` at the CLI layer).
    pub fn source_build_artifact_cache(&self) -> crate::keys::ArtifactCache {
        crate::keys::ArtifactCache::new(self.data.cache_dirs.source_build_artifacts().as_std_path())
    }

    /// Returns the source-build workspace cache rooted at
    /// `cache_dirs.source_build_workspaces()`. Use this to wipe
    /// per-package workspace state so the next build starts from a
    /// clean backend-managed tree.
    pub fn source_build_workspace_cache(&self) -> crate::keys::WorkspaceCache {
        crate::keys::WorkspaceCache::new(
            self.data.cache_dirs.source_build_workspaces().as_std_path(),
        )
    }

    /// Clear all source-build caches (artifacts + workspaces) for the
    /// named package. Silently succeeds on packages that have never
    /// been cached. Suitable for CLI wrappers that implement
    /// `--clean` / `--force-reinstall`.
    pub fn clear_source_build_cache(
        &self,
        package: &rattler_conda_types::PackageName,
    ) -> std::io::Result<()> {
        self.source_build_artifact_cache().clear_package(package)?;
        self.source_build_workspace_cache().clear_package(package)?;
        Ok(())
    }

    /// Returns the gateway used to query conda repodata.
    pub fn gateway(&self) -> &Gateway {
        &self.data.gateway
    }

    /// Returns any build backend overrides.
    pub fn build_backend_overrides(&self) -> &BackendOverride {
        &self.data.build_backend_overrides
    }

    /// Returns the cache directories used by the command dispatcher.
    pub fn cache_dirs(&self) -> &CacheDirs {
        &self.data.cache_dirs
    }

    /// Returns the glob hash cache.
    pub fn glob_hash_cache(&self) -> &GlobHashCache {
        &self.data.glob_hash_cache
    }

    /// Returns the workspace-env registry. Callers allocate a
    /// [`WorkspaceEnvRef`](crate::WorkspaceEnvRef) here and thread the
    /// ref into Keys that carry an
    /// [`EnvironmentRef`](crate::EnvironmentRef). The same registry is
    /// reachable from compute bodies via
    /// `ctx.global_data().workspace_env_registry()`.
    pub fn workspace_env_registry(&self) -> &Arc<WorkspaceEnvRegistry> {
        &self.data.workspace_env_registry
    }

    /// Returns the channel configuration injected into the compute
    /// engine at construction. Panics if no value was injected, matching
    /// the [`pixi_compute_engine::InjectedKey`] contract.
    pub fn channel_config(&self) -> Arc<rattler_conda_types::ChannelConfig> {
        self.engine
            .read(&crate::ChannelConfigKey)
            .expect("ChannelConfig must be injected on the compute engine")
    }

    /// Returns the build-protocol discovery configuration injected into
    /// the compute engine at construction.
    pub fn enabled_protocols(&self) -> Arc<pixi_build_discovery::EnabledProtocols> {
        self.engine
            .read(&crate::EnabledProtocolsKey)
            .expect("EnabledProtocols must be injected on the compute engine")
    }

    /// Discovers the build backend for a source path via the compute
    /// engine. Deduplicated and cached by `DiscoveredBackendKey`; the
    /// channel configuration and enabled protocols come from the
    /// engine-wide injected values.
    pub async fn discovered_backend(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<
        Arc<pixi_build_discovery::DiscoveredBackend>,
        CommandDispatcherError<Arc<pixi_build_discovery::DiscoveryError>>,
    > {
        let key = crate::DiscoveredBackendKey::new(path);
        self.engine
            .with_ctx(async |ctx| ctx.compute(&key).await)
            .await
            .map_err_into_dispatcher(std::convert::identity)
    }

    /// Clears in-memory caches whose correctness depends on the filesystem.
    ///
    /// This invalidates memoized results that are derived from files on disk so
    /// subsequent operations re-check the current state of the filesystem. It:
    /// - clears glob hash memoization (`GlobHashCache`) used for input file hashing
    pub async fn clear_filesystem_caches(&self) {
        self.data.glob_hash_cache.clear();
    }

    /// Returns the download client used by the command dispatcher.
    pub fn download_client(&self) -> &LazyClient {
        &self.data.download_client
    }

    /// Returns the package cache used by the command dispatcher.
    pub fn package_cache(&self) -> &PackageCache {
        &self.data.package_cache
    }

    /// Returns the platform and virtual packages used for tool environments.
    pub fn tool_platform(&self) -> (Platform, &[GenericVirtualPackage]) {
        (self.data.tool_platform.0, &self.data.tool_platform.1)
    }

    /// Returns true if execution of link scripts is enabled.
    pub fn allow_execute_link_scripts(&self) -> bool {
        self.data.execute_link_scripts
    }

    /// Notifies the progress reporter that it should clear its output.
    pub async fn clear_reporter(&self) {
        if let Some(reporter) = self.reporter.as_ref() {
            reporter.on_clear();
        }
    }

    /// Returns the metadata of the source spec.
    ///
    /// Thin wrapper over [`crate::BuildBackendMetadataKey`]; dedup and
    /// caching happen inside the compute engine.
    pub async fn build_backend_metadata(
        &self,
        spec: BuildBackendMetadataSpec,
    ) -> Result<Arc<BuildBackendMetadata>, CommandDispatcherError<BuildBackendMetadataError>> {
        let key = crate::BuildBackendMetadataKey::new(spec);
        self.engine
            .with_ctx(async |ctx| ctx.compute(&key).await)
            .await
            .map_err_into_dispatcher(std::convert::identity)
    }

    /// Returns the metadata for dev sources.
    ///
    /// This method queries the build backend for all outputs from a dev source
    /// and creates DevSourceRecords for each one. These records contain the
    /// combined dependencies (build, host, run) for each output.
    ///
    /// Unlike `source_metadata`, this is specifically for dev sources
    /// where the dependencies are installed but the package itself is not built.
    ///
    /// # Requirements
    ///
    /// - The build backend must support the `conda/outputs` procedure (API v1+)
    pub async fn dev_source_metadata(
        &self,
        spec: DevSourceMetadataSpec,
    ) -> Result<DevSourceMetadata, CommandDispatcherError<DevSourceMetadataError>> {
        let key = crate::DevSourceMetadataKey::new(spec);
        self.engine
            .with_ctx(async |ctx| ctx.compute(&key).await)
            .await
            .map_err_into_dispatcher(std::convert::identity)
            .map(Arc::unwrap_or_clone)
    }

    /// Install a pixi environment.
    ///
    /// This method takes a previously solved environment specification and
    /// installs all required packages into the target prefix. It handles
    /// both binary packages (from conda repositories) and source packages
    /// (built from source code).
    pub async fn install_pixi_environment(
        &self,
        spec: InstallPixiEnvironmentSpec,
    ) -> Result<InstallPixiEnvironmentResult, CommandDispatcherError<InstallPixiEnvironmentError>>
    {
        use crate::command_dispatcher::error::flatten_with_ctx_result;
        use crate::install_pixi::InstallPixiEnvironmentExt;
        // Heap-allocate the inline `with_ctx` + install pipeline so
        // its async state machine does not pile onto the caller's
        // stack. Without the `Box::pin`, a caller that is already
        // deep in another compute pipeline (for example the PyPI
        // resolve path invoking a conda prefix setup for build
        // isolation) can exhaust its thread stack.
        let future: std::pin::Pin<Box<dyn Future<Output = _> + Send + '_>> = Box::pin(
            self.engine
                .with_ctx(async |ctx| ctx.install_pixi_environment(spec).await),
        );
        flatten_with_ctx_result(future.await)
    }

    /// Instantiates an environment for a tool based on the given spec. Reuses
    /// the environment if possible.
    ///
    /// Thin wrapper over [`crate::EphemeralEnvKey`]: builds the key's spec,
    /// adds the `pixi-build-api-version` constraint, computes it, and
    /// extracts the primary-package version + negotiated API from the
    /// installed records.
    pub async fn instantiate_tool_environment(
        &self,
        spec: InstantiateToolEnvironmentSpec,
    ) -> Result<
        InstantiateToolEnvironmentResult,
        CommandDispatcherError<InstantiateToolEnvironmentError>,
    > {
        use pixi_build_types::{
            PIXI_BUILD_API_VERSION_NAME, PIXI_BUILD_API_VERSION_SPEC, PixiBuildApiVersion,
        };
        use pixi_record::PixiRecord;
        use pixi_spec::BinarySpec;

        let primary_name = spec.requirement.0.clone();
        let primary_spec = spec.requirement.1.clone();

        // Merge requirement + additional requirements into one dep map.
        let mut dependencies = spec.additional_requirements.clone();
        dependencies.insert(primary_name.clone(), primary_spec);

        // Append the pixi-build-api-version constraint.
        let mut constraints = spec.constraints.clone();
        constraints.insert(
            PIXI_BUILD_API_VERSION_NAME.clone(),
            BinarySpec::Version(PIXI_BUILD_API_VERSION_SPEC.clone()),
        );

        let ephemeral_spec = crate::EphemeralEnvSpec {
            dependencies,
            constraints,
            channels: spec.channels.clone(),
            exclude_newer: spec.exclude_newer.clone(),
            strategy: Default::default(),
            channel_priority: Default::default(),
        };

        let key = crate::EphemeralEnvKey::new(ephemeral_spec);
        let installed = self
            .engine
            .with_ctx(async |ctx| ctx.compute(&key).await)
            .await
            .map_err_into_dispatcher(InstantiateToolEnvironmentError::EphemeralEnv)?;

        // Extract negotiated API and primary-package version.
        let api = installed
            .records
            .iter()
            .find_map(|r| match r {
                PixiRecord::Binary(b) if b.package_record.name == *PIXI_BUILD_API_VERSION_NAME => {
                    PixiBuildApiVersion::from_version(b.package_record.version.as_ref())
                }
                _ => None,
            })
            .ok_or_else(|| {
                CommandDispatcherError::Failed(
                    InstantiateToolEnvironmentError::NoMatchingBackends {
                        build_backend: Box::new(spec.requirement.clone()),
                    },
                )
            })?;

        let version = installed
            .records
            .iter()
            .find_map(|r| match r {
                PixiRecord::Binary(b) if b.package_record.name == primary_name => {
                    Some(b.package_record.version.clone())
                }
                _ => None,
            })
            .expect("solved env contains the requested primary package");

        Ok(InstantiateToolEnvironmentResult {
            prefix: installed.prefix.clone(),
            version,
            api,
        })
    }

    /// Instantiate (and cache) a build backend handle for the given
    /// [`InstantiateBackendKey`].
    ///
    /// Thin wrapper over the compute-engine Key. Multiple concurrent
    /// callers requesting the same backend share one spawn; the
    /// returned handle wraps the backend in a [`tokio::sync::Mutex`] to
    /// serialize stdio JSON-RPC traffic.
    pub async fn instantiate_backend(
        &self,
        key: InstantiateBackendKey,
    ) -> Result<BackendHandle, CommandDispatcherError<Arc<InstantiateBackendError>>> {
        self.engine
            .with_ctx(async |ctx| ctx.compute(&key).await)
            .await
            .map_err_into_dispatcher(std::convert::identity)
    }
}
