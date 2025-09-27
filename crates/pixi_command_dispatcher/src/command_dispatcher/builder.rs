use std::{path::PathBuf, sync::Arc};

use pixi_build_frontend::BackendOverride;
use pixi_git::resolver::GitResolver;
use pixi_glob::GlobHashCache;
use rattler::package_cache::PackageCache;
use rattler_conda_types::{GenericVirtualPackage, Platform};
use rattler_repodata_gateway::{Gateway, MaxConcurrency};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use reqwest_middleware::ClientWithMiddleware;

use crate::build::source_metadata_cache::SourceMetadataCache;
use crate::discover_backend_cache::DiscoveryCache;
use crate::{
    CacheDirs, CommandDispatcher, Executor, Limits, Reporter,
    build::BuildCache,
    command_dispatcher::{CommandDispatcherChannel, CommandDispatcherData},
    command_dispatcher_processor::CommandDispatcherProcessor,
    limits::ResolvedLimits,
};

#[derive(Default)]
pub struct CommandDispatcherBuilder {
    gateway: Option<Gateway>,
    root_dir: Option<PathBuf>,
    reporter: Option<Box<dyn Reporter>>,
    git_resolver: Option<GitResolver>,
    download_client: Option<ClientWithMiddleware>,
    cache_dirs: Option<CacheDirs>,
    build_backend_overrides: BackendOverride,
    max_download_concurrency: MaxConcurrency,
    limits: Limits,
    executor: Executor,
    tool_platform: Option<(Platform, Vec<GenericVirtualPackage>)>,
    execute_link_scripts: bool,
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
            reporter: Some(Box::new(reporter)),
            ..self
        }
    }

    /// Sets the reqwest client to use for network fetches.
    pub fn with_download_client(self, client: ClientWithMiddleware) -> Self {
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

    /// Sets the root directory for resolving relative paths.
    pub fn with_root_dir(self, root_dir: PathBuf) -> Self {
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

    /// Completes the builder and returns a new [`CommandDispatcher`].
    pub fn finish(self) -> CommandDispatcher {
        let root_dir = self
            .root_dir
            .or(std::env::current_dir().ok())
            .unwrap_or_default();
        let cache_dirs = self
            .cache_dirs
            .unwrap_or_else(|| CacheDirs::new(root_dir.join(".cache")));
        let download_client = self.download_client.unwrap_or_default();
        let package_cache = PackageCache::new(cache_dirs.packages());
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(download_client.clone())
                .with_cache_dir(cache_dirs.root().clone())
                .with_package_cache(package_cache.clone())
                .with_max_concurrent_requests(self.max_download_concurrency)
                .finish()
        });

        let git_resolver = self.git_resolver.unwrap_or_default();
        let source_metadata_cache = SourceMetadataCache::new(cache_dirs.source_metadata());
        let build_cache = BuildCache::new(cache_dirs.source_builds());
        let tool_platform = self.tool_platform.unwrap_or_else(|| {
            let platform = Platform::current();
            let virtual_packages =
                VirtualPackages::detect(&VirtualPackageOverrides::default()).unwrap_or_default();
            (
                platform,
                virtual_packages.into_generic_virtual_packages().collect(),
            )
        });

        let data = Arc::new(CommandDispatcherData {
            gateway,
            source_metadata_cache,
            build_cache,
            root_dir,
            git_resolver,
            cache_dirs,
            download_client,
            build_backend_overrides: self.build_backend_overrides,
            glob_hash_cache: GlobHashCache::default(),
            discovery_cache: DiscoveryCache::default(),
            limits: ResolvedLimits::from(self.limits),
            package_cache,
            tool_platform,
            execute_link_scripts: self.execute_link_scripts,
            executor: self.executor,
        });

        let (sender, join_handle) = CommandDispatcherProcessor::spawn(data.clone(), self.reporter);
        CommandDispatcher {
            channel: Some(CommandDispatcherChannel::Strong(sender)),
            context: None,
            data,
            processor_handle: Some(Arc::new(join_handle)),
        }
    }
}
