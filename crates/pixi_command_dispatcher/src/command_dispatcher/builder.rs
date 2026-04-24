use std::sync::Arc;

use crate::BuildEnvironment;
use crate::cache::build_backend_metadata::BuildBackendMetadataCache;
use crate::compute_data::{
    AllowExecuteLinkScripts, BackendSourceBuildSemaphore, CondaSolveSemaphore,
};
use crate::environment::WorkspaceEnvRegistry;
use crate::injected_config::{
    BackendOverrideKey, ChannelConfigKey, EnabledProtocolsKey, ToolBuildEnvironmentKey,
};
use crate::path::RootDir;
use crate::reporter_context::CURRENT_REPORTER_CONTEXT;
use crate::{
    CacheDirs, CommandDispatcher, Executor, Limits, Reporter,
    command_dispatcher::{CommandDispatcherData, DepGraphDumpGuard},
    limits::ResolvedLimits,
    source_checkout::{GitCheckoutSemaphore, UrlCheckoutSemaphore},
};
use futures::future::BoxFuture;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_frontend::BackendOverride;
use pixi_compute_engine::{ComputeEngine, DataStore, SpawnHook};
use pixi_git::resolver::GitResolver;
use pixi_glob::GlobHashCache;
use pixi_path::{AbsPathBuf, AbsPresumedDirPathBuf};
use pixi_url::resolver::UrlResolver;
use rattler::package_cache::PackageCache;
use rattler_conda_types::{ChannelConfig, GenericVirtualPackage, Platform};
use rattler_networking::LazyClient;
use rattler_repodata_gateway::{Gateway, MaxConcurrency};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use tokio::sync::Semaphore;

#[derive(Default)]
pub struct CommandDispatcherBuilder {
    gateway: Option<Gateway>,
    root_dir: Option<AbsPresumedDirPathBuf>,
    reporter: Option<Arc<dyn Reporter>>,
    git_resolver: Option<GitResolver>,
    url_resolver: Option<UrlResolver>,
    download_client: Option<LazyClient>,
    cache_dirs: Option<CacheDirs>,
    build_backend_overrides: BackendOverride,
    max_download_concurrency: MaxConcurrency,
    limits: Limits,
    executor: Executor,
    tool_platform: Option<(Platform, Vec<GenericVirtualPackage>)>,
    execute_link_scripts: bool,
    channel_config: Option<ChannelConfig>,
    enabled_protocols: Option<EnabledProtocols>,
}

impl CommandDispatcherBuilder {
    /// Sets the cache directories to use.
    pub fn with_cache_dirs(self, cache_dirs: CacheDirs) -> Self {
        Self {
            cache_dirs: Some(cache_dirs),
            ..self
        }
    }

    /// Sets the gateway to use for querying conda repodata.
    pub fn with_gateway(self, gateway: Gateway) -> Self {
        Self {
            gateway: Some(gateway),
            ..self
        }
    }

    /// Sets the reporter used by the [`CommandDispatcher`] to report progress.
    pub fn with_reporter<F: Reporter + 'static>(self, reporter: F) -> Self {
        Self {
            reporter: Some(Arc::new(reporter)),
            ..self
        }
    }

    /// Sets the reqwest client to use for network fetches.
    pub fn with_download_client(self, client: LazyClient) -> Self {
        Self {
            download_client: Some(client),
            ..self
        }
    }

    /// Sets the git resolver used to fetch git repositories.
    pub fn with_git_resolver(self, resolver: GitResolver) -> Self {
        Self {
            git_resolver: Some(resolver),
            ..self
        }
    }

    /// Sets the url resolver used to fetch archives.
    pub fn with_url_resolver(self, resolver: UrlResolver) -> Self {
        Self {
            url_resolver: Some(resolver),
            ..self
        }
    }

    /// Sets the root directory for resolving relative paths.
    pub fn with_root_dir(self, root_dir: AbsPresumedDirPathBuf) -> Self {
        Self {
            root_dir: Some(root_dir),
            ..self
        }
    }

    /// Apply overrides to particular backends.
    pub fn with_backend_overrides(self, overrides: BackendOverride) -> Self {
        Self {
            build_backend_overrides: overrides,
            ..self
        }
    }

    /// Sets the maximum number of concurrent downloads.
    pub fn with_max_download_concurrency(self, max_concurrency: impl Into<MaxConcurrency>) -> Self {
        Self {
            max_download_concurrency: max_concurrency.into(),
            ..self
        }
    }

    /// Sets the tool platform and virtual packages associated with it. This is
    /// used when instantiating tool environments and defaults to the
    /// current platform.
    pub fn with_tool_platform(
        self,
        platform: Platform,
        virtual_packages: Vec<GenericVirtualPackage>,
    ) -> Self {
        Self {
            tool_platform: Some((platform, virtual_packages)),
            ..self
        }
    }

    /// Set the limits to which this instance should adhere.
    pub fn with_limits(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }

    /// Sets the executor to use for the command dispatcher.
    pub fn with_executor(self, executor: Executor) -> Self {
        Self { executor, ..self }
    }

    /// Whether to allow executing link scripts when installing packages.
    pub fn execute_link_scripts(self, execute: bool) -> Self {
        Self {
            execute_link_scripts: execute,
            ..self
        }
    }

    /// Sets the channel configuration used to resolve channel names.
    /// Injected into the compute engine as [`ChannelConfigKey`].
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config: Some(channel_config),
            ..self
        }
    }

    /// Sets the build-protocol discovery configuration. Injected into
    /// the compute engine as [`EnabledProtocolsKey`].
    pub fn with_enabled_protocols(self, enabled_protocols: EnabledProtocols) -> Self {
        Self {
            enabled_protocols: Some(enabled_protocols),
            ..self
        }
    }

    /// Completes the builder and returns a new [`CommandDispatcher`].
    pub fn finish(self) -> CommandDispatcher {
        let root_dir = self.root_dir.unwrap_or_else(|| {
            let current_dir =
                std::env::current_dir().expect("failed to determine current directory");
            AbsPathBuf::new(current_dir)
                .expect("current directory is not an absolute path")
                .into_assume_dir()
        });

        let cache_dirs = self
            .cache_dirs
            .unwrap_or_else(|| CacheDirs::new(root_dir.join(".cache").into_assume_dir()));
        let download_client = self.download_client.unwrap_or_default();
        let package_cache = PackageCache::new(cache_dirs.packages());
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(download_client.clone())
                .with_cache_dir(cache_dirs.root().to_owned().into_std_path_buf())
                .with_package_cache(package_cache.clone())
                .with_max_concurrent_requests(self.max_download_concurrency)
                .finish()
        });

        let git_resolver = self.git_resolver.unwrap_or_default();
        let build_backend_metadata_cache =
            BuildBackendMetadataCache::new(cache_dirs.backend_metadata().into());

        let url_resolver = self.url_resolver.unwrap_or_default();

        let tool_platform = self.tool_platform.unwrap_or_else(|| {
            let platform = Platform::current();
            let virtual_packages =
                VirtualPackages::detect(&VirtualPackageOverrides::default()).unwrap_or_default();
            (
                platform,
                virtual_packages.into_generic_virtual_packages().collect(),
            )
        });

        let limits = ResolvedLimits::from(self.limits);
        let git_checkout_semaphore = limits
            .max_concurrent_git_checkouts
            .map(|n| Arc::new(Semaphore::new(n)));
        let url_checkout_semaphore = limits
            .max_concurrent_url_checkouts
            .map(|n| Arc::new(Semaphore::new(n)));
        let conda_solve_semaphore = limits
            .max_concurrent_solves
            .map(|n| Arc::new(Semaphore::new(n)));
        let backend_source_build_semaphore = limits
            .max_concurrent_builds
            .map(|n| Arc::new(Semaphore::new(n)));

        let reporter = self.reporter;

        let channel_config = self.channel_config.unwrap_or_else(|| {
            let path: &std::path::Path = root_dir.as_ref();
            ChannelConfig::default_with_root_dir(path.to_path_buf())
        });
        let enabled_protocols = self.enabled_protocols.unwrap_or_default();

        let workspace_env_registry = Arc::new(WorkspaceEnvRegistry::new());

        let data = Arc::new(CommandDispatcherData {
            gateway,
            build_backend_metadata_cache,
            git_resolver,
            url_resolver,
            cache_dirs,
            download_client,
            build_backend_overrides: self.build_backend_overrides,
            glob_hash_cache: GlobHashCache::default(),
            package_cache,
            tool_platform,
            execute_link_scripts: self.execute_link_scripts,
            executor: self.executor,
            git_checkout_semaphore,
            url_checkout_semaphore,
            conda_solve_semaphore,
            backend_source_build_semaphore,
            workspace_env_registry,
        });

        // Build the compute engine, populating its global data store with
        // the individual shared resources that pixi-specific Keys read
        // through the extension traits on DataStore (HasGateway,
        // HasGitResolver, HasUrlResolver, HasCacheDirs, ...). Values are
        // stored by `TypeId`, so tests can populate only the subset their
        // Keys actually touch without constructing a full dispatcher. The
        // spawn hook captures the calling task's
        // `CURRENT_REPORTER_CONTEXT` task-local and scopes the spawned
        // compute with it, so Keys can read their caller's reporter
        // context even though the compute runs on a fresh tokio task.
        let mut engine_builder = ComputeEngine::builder()
            .sequential_branches(matches!(self.executor, Executor::Serial))
            .with_data(data.gateway.clone())
            .with_data(data.git_resolver.clone())
            .with_data(data.url_resolver.clone())
            .with_data(data.download_client.clone())
            .with_data(data.cache_dirs.clone())
            .with_data(data.build_backend_metadata_cache.clone())
            .with_data(data.package_cache.clone())
            .with_data(data.workspace_env_registry.clone())
            .with_data(AllowExecuteLinkScripts(data.execute_link_scripts))
            .with_data(RootDir(root_dir))
            .with_spawn_hook(Arc::new(ReporterContextSpawnHook));
        if let Some(reporter) = reporter.clone() {
            engine_builder = engine_builder.with_data(reporter);
        }
        if let Some(sem) = data.git_checkout_semaphore.clone() {
            engine_builder = engine_builder.with_data(GitCheckoutSemaphore(sem));
        }
        if let Some(sem) = data.url_checkout_semaphore.clone() {
            engine_builder = engine_builder.with_data(UrlCheckoutSemaphore(sem));
        }
        if let Some(sem) = data.conda_solve_semaphore.clone() {
            engine_builder = engine_builder.with_data(CondaSolveSemaphore(sem));
        }
        if let Some(sem) = data.backend_source_build_semaphore.clone() {
            engine_builder = engine_builder.with_data(BackendSourceBuildSemaphore(sem));
        }
        let engine = engine_builder.build();

        // Inject engine-wide configuration values that Keys read through
        // `ctx.compute(&ChannelConfigKey)` etc.
        engine.inject(ChannelConfigKey, Arc::new(channel_config));
        engine.inject(EnabledProtocolsKey, Arc::new(enabled_protocols));
        let tool_build_environment = BuildEnvironment {
            host_platform: data.tool_platform.0,
            build_platform: data.tool_platform.0,
            host_virtual_packages: data.tool_platform.1.clone(),
            build_virtual_packages: data.tool_platform.1.clone(),
        };
        engine.inject(ToolBuildEnvironmentKey, Arc::new(tool_build_environment));
        engine.inject(
            BackendOverrideKey,
            Arc::new(data.build_backend_overrides.clone()),
        );

        CommandDispatcher {
            _dump_guard: Arc::new(DepGraphDumpGuard {
                engine: engine.clone(),
            }),
            data,
            engine,
            reporter,
        }
    }
}

/// Snapshots the current reporter-context task-local on the calling
/// task and re-installs it via `scope` on the spawned compute task.
/// Lets compute-engine Keys read the caller's reporter context even
/// though the compute runs on a fresh tokio task that would not
/// otherwise inherit task-locals.
pub struct ReporterContextSpawnHook;

impl SpawnHook for ReporterContextSpawnHook {
    fn wrap(&self, _data: &DataStore, fut: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        let captured = CURRENT_REPORTER_CONTEXT.try_get().ok().flatten();
        Box::pin(CURRENT_REPORTER_CONTEXT.scope(captured, fut))
    }
}
