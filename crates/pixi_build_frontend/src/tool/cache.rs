use std::{fmt::Debug, hash::Hash, path::PathBuf};

use dashmap::{DashMap, Entry};
use miette::{miette, IntoDiagnostic, Result};
use pixi_consts::consts::{CACHED_BUILD_ENVS_DIR, CONDA_REPODATA_CACHE_DIR};
use pixi_progress::wrap_in_progress;
use pixi_utils::{EnvironmentHash, PrefixGuard};
use rattler::{install::Installer, package_cache::PackageCache};
use rattler_conda_types::{Channel, ChannelConfig, GenericVirtualPackage, Platform};
use rattler_repodata_gateway::Gateway;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::ClientWithMiddleware;

use super::IsolatedTool;
use crate::{
    tool::{SystemTool, Tool, ToolSpec},
    IsolatedToolSpec, SystemToolSpec,
};

pub struct ToolContextBuilder {
    gateway: Option<Gateway>,
    client: ClientWithMiddleware,
    cache_dir: PathBuf,
    cache: ToolCache,
    platform: Platform,
}

impl Default for ToolContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolContextBuilder {
    /// Create a new tool context builder.
    pub fn new() -> Self {
        Self {
            gateway: None,
            client: ClientWithMiddleware::default(),
            cache_dir: pixi_config::get_cache_dir().expect("we should have a cache dir"),
            cache: ToolCache::default(),
            platform: Platform::current(),
        }
    }

    /// Set the platform to install tools for. This is usually the current
    /// platform but could also be a compatible platform. For instance if the
    /// current platform is win-arm64, the compatible platform could be win-64.
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Set the gateway for the tool context.
    pub fn with_gateway(mut self, gateway: Gateway) -> Self {
        self.gateway = Some(gateway);
        self
    }

    /// Set the client for the tool context.
    pub fn with_client(mut self, client: ClientWithMiddleware) -> Self {
        self.client = client;
        self
    }

    /// Set the cache directory for the tool context.
    pub fn with_cache_dir(mut self, cache_dir: PathBuf) -> Self {
        self.cache_dir = cache_dir;
        self
    }

    pub fn with_cache(mut self, cache: ToolCache) -> Self {
        self.cache = cache;
        self
    }

    /// Build the `ToolContext` using builder configuration.
    pub fn build(self) -> ToolContext {
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(self.client.clone())
                .with_cache_dir(self.cache_dir.join(CONDA_REPODATA_CACHE_DIR))
                .finish()
        });

        ToolContext {
            cache_dir: self.cache_dir,
            client: self.client,
            cache: self.cache,
            platform: self.platform,
            gateway,
        }
    }
}

/// The tool context,
/// containing client, channels and gateway configuration
/// that will be used to resolve and install tools.
pub struct ToolContext {
    /// Authentication client to use for fetching repodata.
    pub client: ClientWithMiddleware,
    /// The cache directory to use for the tools.
    pub cache_dir: PathBuf,
    /// The gateway to use for fetching repodata.
    pub gateway: Gateway,
    /// The cache to use for the tools.
    pub cache: ToolCache,
    /// The platform to install tools for. This is usually the current platform
    /// but could also be a compatible platform. For instance if the current
    /// platform is win-arm64, the compatible platform could be win-64.
    pub platform: Platform,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("client", &self.client)
            .field("cache_dir", &self.cache_dir)
            .finish()
    }
}

impl ToolContext {
    /// Create a new tool context builder with the given channels.
    pub fn builder() -> ToolContextBuilder {
        ToolContextBuilder::new()
    }

    /// Instantiate a tool from a specification.
    ///
    /// If the tool is not already cached, it will be created, installed and cached.
    pub async fn instantiate(
        &self,
        spec: ToolSpec,
        channel_config: &ChannelConfig,
    ) -> Result<Tool, ToolCacheError> {
        let spec = match spec {
            ToolSpec::Io(ipc) => return Ok(Tool::Io(ipc)),
            ToolSpec::Isolated(isolated) => CacheableToolSpec::Isolated(isolated),
            ToolSpec::System(system) => CacheableToolSpec::System(system),
        };

        let cache_entry = match self.cache.cache.entry(spec.clone()) {
            Entry::Occupied(entry) => return Ok(entry.get().clone().into()),
            Entry::Vacant(entry) => entry,
        };

        let tool: CachedTool = match spec {
            CacheableToolSpec::Isolated(spec) => CachedTool::Isolated(if spec.specs.is_empty() {
                return Err(ToolCacheError::Install(miette!(
                    "No build match specs provided for '{}' command.",
                    spec.command
                )));
            } else {
                self.install(&spec, channel_config)
                    .await
                    .map_err(ToolCacheError::Install)?
            }),
            CacheableToolSpec::System(spec) => SystemTool::new(spec.command).into(),
        };

        cache_entry.insert(tool.clone());
        Ok(tool.into())
    }

    /// Installed the tool in the isolated environment.
    pub async fn install(
        &self,
        spec: &IsolatedToolSpec,
        channel_config: &ChannelConfig,
    ) -> miette::Result<IsolatedTool> {
        let channels: Vec<Channel> = spec
            .channels
            .iter()
            .cloned()
            .map(|channel| channel.into_channel(channel_config))
            .collect::<Result<Vec<Channel>, _>>()
            .into_diagnostic()?;

        let repodata = self
            .gateway
            .query(
                channels.clone(),
                [self.platform, Platform::NoArch],
                spec.specs.clone(),
            )
            .recursive(true)
            .execute()
            .await
            .into_diagnostic()?;

        // Determine virtual packages of the current platform
        let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::from_env())
            .unwrap()
            .iter()
            .cloned()
            .map(GenericVirtualPackage::from)
            .collect();

        let solved_records = Solver
            .solve(SolverTask {
                specs: spec.specs.clone(),
                virtual_packages,
                ..SolverTask::from_iter(&repodata)
            })
            .into_diagnostic()?;

        let cache = EnvironmentHash::new(
            spec.command.clone(),
            spec.specs.clone(),
            channels.iter().map(|c| c.base_url.to_string()).collect(),
            self.platform,
        );

        let cached_dir = self
            .cache_dir
            .join(CACHED_BUILD_ENVS_DIR)
            .join(cache.name());

        let mut prefix_guard = PrefixGuard::new(&cached_dir).into_diagnostic()?;

        let mut write_guard =
            wrap_in_progress("acquiring write lock on prefix", || prefix_guard.write())
                .into_diagnostic()?;

        // If the environment already exists, we can return early.
        if write_guard.is_ready() {
            tracing::info!("reusing existing environment in {}", cached_dir.display());
            let _ = write_guard.finish();

            // Get the activation scripts
            let activator =
                Activator::from_path(&cached_dir, ShellEnum::default(), Platform::current())
                    .unwrap();

            let activation_scripts = activator
                .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
                .unwrap();

            return Ok(IsolatedTool::new(
                spec.command.clone(),
                cached_dir,
                activation_scripts,
            ));
        }

        // Update the prefix to indicate that we are installing it.
        write_guard.begin().into_diagnostic()?;

        // Install the environment
        Installer::new()
            .with_target_platform(self.platform)
            .with_download_client(self.client.clone())
            .with_package_cache(PackageCache::new(
                self.cache_dir
                    .join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
            ))
            .install(&cached_dir, solved_records)
            .await
            .into_diagnostic()?;

        // Get the activation scripts
        let activator =
            Activator::from_path(&cached_dir, ShellEnum::default(), self.platform).unwrap();

        let activation_scripts = activator
            .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
            .unwrap();

        let _ = write_guard.finish();

        Ok(IsolatedTool::new(
            spec.command.clone(),
            cached_dir,
            activation_scripts,
        ))
    }
}

/// A [`ToolCache`] maintains a cache of environments for build tools.
///
/// This is useful to ensure that if we need to build multiple packages that use
/// the same tool, we can reuse their environments.
/// (nichita): it can also be seen as a way to create tools itself
#[derive(Default, Debug)]
pub struct ToolCache {
    /// The cache of tools.
    pub cache: DashMap<CacheableToolSpec, CachedTool>,
}

#[derive(thiserror::Error, Debug)]
pub enum ToolCacheError {
    #[error("could not resolve '{}', {1}", .0.display())]
    Instantiate(PathBuf, which::Error),
    #[error("could not install isolated tool '{}'", .0.as_display())]
    Install(miette::Report),
    #[error("could not determine default cache dir '{}'", .0.as_display())]
    CacheDir(miette::Report),
}

/// Describes the specification of the tool. This can be used to cache tool
/// information.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum CacheableToolSpec {
    Isolated(IsolatedToolSpec),
    System(SystemToolSpec),
}

/// A tool that can be invoked.
#[derive(Debug, Clone)]
pub enum CachedTool {
    Isolated(IsolatedTool),
    System(SystemTool),
}

impl From<CachedTool> for Tool {
    fn from(value: CachedTool) -> Self {
        match value {
            CachedTool::Isolated(tool) => Tool::Isolated(tool),
            CachedTool::System(tool) => Tool::System(tool),
        }
    }
}

impl From<IsolatedTool> for CachedTool {
    fn from(value: IsolatedTool) -> Self {
        Self::Isolated(value)
    }
}

impl From<SystemTool> for CachedTool {
    fn from(value: SystemTool) -> Self {
        Self::System(value)
    }
}

impl ToolCache {
    /// Construct a new tool cache.
    pub fn new() -> Self {
        Self {
            cache: DashMap::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use pixi_config::Config;
    use rattler_conda_types::{MatchSpec, ParseStrictness, Platform};
    use reqwest_middleware::ClientWithMiddleware;

    // use super::ToolCache;
    use crate::{
        tool::{ToolContext, ToolSpec},
        IsolatedToolSpec,
    };

    /// Returns the platform to use for the tool cache. Python is not yet
    /// available for win-arm64 so we use win-64.
    pub fn compatible_target_platform() -> Platform {
        match Platform::current() {
            Platform::WinArm64 => Platform::Win64,
            platform => platform,
        }
    }

    #[tokio::test]
    async fn test_tool_cache() {
        let config = Config::for_tests();

        let auth_client = ClientWithMiddleware::default();
        let channel_config = config.global_channel_config();

        let tool_context = ToolContext::builder()
            .with_platform(compatible_target_platform())
            .with_client(auth_client.clone())
            .with_gateway(config.gateway(auth_client))
            .build();

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("bat", ParseStrictness::Strict).unwrap()],
            command: "bat".into(),
            channels: config.default_channels.clone(),
        };

        let tool = tool_context
            .instantiate(ToolSpec::Isolated(tool_spec), channel_config)
            .await
            .unwrap();

        let exec = tool.as_executable().unwrap();

        exec.command().arg("--version").spawn().unwrap();
    }
}
