use std::{collections::HashMap, sync::Arc};

use crate::BuildEnvironment;
use crate::cache::{
    BuildBackendMetadataCache,
    markers::{BackendMetadataDir, PackagesDir},
};
use crate::compute_data::{
    AllowExecuteLinkScripts, AllowLinkOptions, BackendSourceBuildSemaphore, CondaSolveSemaphore,
};
use crate::environment::WorkspaceEnvRegistry;
use crate::injected_config::{
    BackendOverrideKey, ChannelConfigKey, EnabledProtocolsKey, ToolBuildEnvironmentKey,
};
use crate::reporter::{
    BackendSourceBuildReporter, BuildBackendMetadataReporter, CondaSolveReporter,
    GitCheckoutReporter, InstantiateBackendReporter, PixiInstallReporter, PixiSolveReporter,
    SourceMetadataReporter, SourceRecordReporter, UrlCheckoutReporter,
};
use crate::util::limits::ResolvedLimits;
use crate::util::path::RootDir;
use crate::{
    CacheDirs, CommandDispatcher, Executor, Limits,
    command_dispatcher::{CommandDispatcherData, DepGraphDumpGuard},
    source_checkout::{GitCheckoutSemaphore, UrlCheckoutSemaphore},
};
use pixi_build_discovery::EnabledProtocols;
use pixi_build_frontend::BackendOverride;
use pixi_compute_cache_dirs::CacheDirsKey;
use pixi_compute_engine::ComputeEngine;
use pixi_compute_env_vars::EnvVarsKey;
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
    /// Allow symbolic links during package installation.
    allow_symbolic_links: Option<bool>,
    /// Allow hard links during package installation.
    allow_hard_links: Option<bool>,
    /// Allow ref links (copy-on-write) during package installation.
    allow_ref_links: Option<bool>,

    // Per-key reporters; each registered separately into the engine
    // `DataStore` at `finish()` so per-key compute bodies can read just
    // the reporter they need without depending on a single umbrella
    // trait.
    pixi_install_reporter: Option<Arc<dyn PixiInstallReporter>>,
    pixi_solve_reporter: Option<Arc<dyn PixiSolveReporter>>,
    conda_solve_reporter: Option<Arc<dyn CondaSolveReporter>>,
    git_checkout_reporter: Option<Arc<dyn GitCheckoutReporter>>,
    url_checkout_reporter: Option<Arc<dyn UrlCheckoutReporter>>,
    instantiate_backend_reporter: Option<Arc<dyn InstantiateBackendReporter>>,
    build_backend_metadata_reporter: Option<Arc<dyn BuildBackendMetadataReporter>>,
    source_metadata_reporter: Option<Arc<dyn SourceMetadataReporter>>,
    source_record_reporter: Option<Arc<dyn SourceRecordReporter>>,
    backend_source_build_reporter: Option<Arc<dyn BackendSourceBuildReporter>>,
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

    /// Register the per-key
    /// [`PixiInstallReporter`](crate::PixiInstallReporter) used by the
    /// install-pixi-environment path.
    pub fn with_pixi_install_reporter(self, reporter: Arc<dyn PixiInstallReporter>) -> Self {
        Self {
            pixi_install_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`PixiSolveReporter`](crate::PixiSolveReporter) used by
    /// [`SolvePixiEnvironmentKey`](crate::keys::SolvePixiEnvironmentKey).
    pub fn with_pixi_solve_reporter(self, reporter: Arc<dyn PixiSolveReporter>) -> Self {
        Self {
            pixi_solve_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`CondaSolveReporter`](crate::CondaSolveReporter) used by the
    /// conda-solve path.
    pub fn with_conda_solve_reporter(self, reporter: Arc<dyn CondaSolveReporter>) -> Self {
        Self {
            conda_solve_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`GitCheckoutReporter`](crate::GitCheckoutReporter) used by the
    /// git-checkout key.
    pub fn with_git_checkout_reporter(self, reporter: Arc<dyn GitCheckoutReporter>) -> Self {
        Self {
            git_checkout_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`UrlCheckoutReporter`](crate::reporter::UrlCheckoutReporter) used
    /// by the url-checkout key.
    pub fn with_url_checkout_reporter(self, reporter: Arc<dyn UrlCheckoutReporter>) -> Self {
        Self {
            url_checkout_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`InstantiateBackendReporter`](crate::InstantiateBackendReporter)
    /// used by [`InstantiateBackendKey`](crate::InstantiateBackendKey).
    pub fn with_instantiate_backend_reporter(
        self,
        reporter: Arc<dyn InstantiateBackendReporter>,
    ) -> Self {
        Self {
            instantiate_backend_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`BuildBackendMetadataReporter`](crate::BuildBackendMetadataReporter)
    /// used by
    /// [`BuildBackendMetadataKey`](crate::BuildBackendMetadataKey).
    pub fn with_build_backend_metadata_reporter(
        self,
        reporter: Arc<dyn BuildBackendMetadataReporter>,
    ) -> Self {
        Self {
            build_backend_metadata_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`SourceMetadataReporter`](crate::SourceMetadataReporter) used by
    /// [`ResolveSourcePackageKey`](crate::keys::ResolveSourcePackageKey).
    pub fn with_source_metadata_reporter(self, reporter: Arc<dyn SourceMetadataReporter>) -> Self {
        Self {
            source_metadata_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`SourceRecordReporter`](crate::SourceRecordReporter) used by
    /// `assemble_source_record` (the per-variant fan-out under
    /// [`ResolveSourcePackageKey`](crate::keys::ResolveSourcePackageKey)).
    pub fn with_source_record_reporter(self, reporter: Arc<dyn SourceRecordReporter>) -> Self {
        Self {
            source_record_reporter: Some(reporter),
            ..self
        }
    }

    /// Register the per-key
    /// [`BackendSourceBuildReporter`](crate::BackendSourceBuildReporter)
    /// used by the backend-source-build path inside
    /// [`SourceBuildKey`](crate::keys::SourceBuildKey).
    pub fn with_backend_source_build_reporter(
        self,
        reporter: Arc<dyn BackendSourceBuildReporter>,
    ) -> Self {
        Self {
            backend_source_build_reporter: Some(reporter),
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

    /// Sets whether symbolic links are allowed during package installation.
    pub fn with_allow_symbolic_links(self, allow: Option<bool>) -> Self {
        Self {
            allow_symbolic_links: allow,
            ..self
        }
    }

    /// Sets whether hard links are allowed during package installation.
    pub fn with_allow_hard_links(self, allow: Option<bool>) -> Self {
        Self {
            allow_hard_links: allow,
            ..self
        }
    }

    /// Sets whether ref links (copy-on-write) are allowed during package installation.
    pub fn with_allow_ref_links(self, allow: Option<bool>) -> Self {
        Self {
            allow_ref_links: allow,
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

        let cache_dirs = Arc::new(
            self.cache_dirs
                .unwrap_or_else(|| CacheDirs::new(root_dir.join(".cache").into_assume_dir())),
        );
        // Snapshot env vars once. Reused for the sync resolves below
        // and injected via `EnvVarsKey` so compute bodies see the same
        // map.
        let env_snapshot: Arc<HashMap<String, String>> = Arc::new(std::env::vars().collect());

        let download_client = self.download_client.unwrap_or_default();
        let package_cache =
            PackageCache::new(cache_dirs.resolve_with_env::<PackagesDir>(&env_snapshot));
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(download_client.clone())
                .with_cache_dir(cache_dirs.root().to_owned().into_std_path_buf())
                .with_package_cache(package_cache.clone())
                .with_max_concurrent_requests(self.max_download_concurrency)
                .finish()
        });

        let git_resolver = self.git_resolver.unwrap_or_default();
        let build_backend_metadata_cache = BuildBackendMetadataCache::new(
            cache_dirs
                .resolve_with_env::<BackendMetadataDir>(&env_snapshot)
                .into(),
        );

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
            allow_symbolic_links: self.allow_symbolic_links,
            allow_hard_links: self.allow_hard_links,
            allow_ref_links: self.allow_ref_links,
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
        // HasGitResolver, HasUrlResolver, ...). Values are stored by
        // `TypeId`, so tests can populate only the subset their Keys
        // actually touch without constructing a full dispatcher.
        // CacheDirs and the env-var snapshot are injected separately
        // via `CacheDirsKey` and `EnvVarsKey` so consumers depend on
        // them through the engine graph.
        let mut engine_builder = ComputeEngine::builder()
            .sequential_branches(matches!(self.executor, Executor::Serial))
            .with_data(data.gateway.clone())
            .with_data(data.git_resolver.clone())
            .with_data(data.url_resolver.clone())
            .with_data(data.download_client.clone())
            .with_data(data.build_backend_metadata_cache.clone())
            .with_data(data.package_cache.clone())
            .with_data(data.workspace_env_registry.clone())
            .with_data(AllowExecuteLinkScripts(data.execute_link_scripts))
            .with_data(AllowLinkOptions {
                allow_symbolic_links: data.allow_symbolic_links,
                allow_hard_links: data.allow_hard_links,
                allow_ref_links: data.allow_ref_links,
            })
            .with_data(RootDir(root_dir))
            .with_spawn_hook(Arc::new(pixi_compute_reporters::OperationIdSpawnHook));
        // Register each per-key reporter the caller supplied; a missing
        // reporter is treated as "no progress UI for this kind of work."
        if let Some(r) = self.pixi_install_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.pixi_solve_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.conda_solve_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.git_checkout_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.url_checkout_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.instantiate_backend_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.build_backend_metadata_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.source_metadata_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.source_record_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
        }
        if let Some(r) = self.backend_source_build_reporter.clone() {
            engine_builder = engine_builder.with_data(r);
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
        engine.inject(CacheDirsKey, data.cache_dirs.clone());
        engine.inject(EnvVarsKey, env_snapshot);
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
        }
    }
}
