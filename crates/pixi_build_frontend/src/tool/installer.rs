use std::{fmt::Debug, future::Future, path::PathBuf};

use miette::{IntoDiagnostic, miette};
use pixi_build_types::{PIXI_BUILD_API_VERSION_NAME, PIXI_BUILD_API_VERSION_SPEC};
use pixi_consts::consts::CACHED_BUILD_TOOL_ENVS_DIR;
use pixi_progress::await_in_progress;
use pixi_utils::{AsyncPrefixGuard, EnvironmentHash};
use rattler::{install::Installer, package_cache::PackageCache};
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, NamelessMatchSpec, Platform,
};
use rattler_repodata_gateway::Gateway;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::ClientWithMiddleware;

use super::{
    IsolatedTool, IsolatedToolSpec, SystemTool, Tool, ToolCacheError, ToolSpec, cache::ToolCache,
};

/// A trait that is responsible for installing tools.
pub trait ToolInstaller {
    /// Install the tool.
    fn install(
        &self,
        tool: &IsolatedToolSpec,
        channel_config: &ChannelConfig,
    ) -> impl Future<Output = miette::Result<IsolatedTool>> + Send;
}

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
                .with_cache_dir(self.cache_dir.clone())
                .with_client(self.client.clone())
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
    // Authentication client to use for fetching repodata.
    pub client: ClientWithMiddleware,
    // The cache directory to use while installing tools.
    // This cache directory is also passed to the Gateway and Installer.
    pub cache_dir: PathBuf,
    // The gateway to use for fetching repodata.
    pub gateway: Gateway,
    // The cache to use for the tools.
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
            .field("platform", &self.platform)
            .finish()
    }
}

impl ToolContext {
    /// Create a new tool context builder with the given channels.
    pub fn builder() -> ToolContextBuilder {
        ToolContextBuilder::new()
    }

    /// Create a new tool context builder to be used for tests
    ///
    /// The main difference is that it uses a tmp cache directory
    /// instead of rattler one
    #[cfg(test)]
    pub fn for_tests() -> ToolContextBuilder {
        let cache_dir = tempfile::tempdir().unwrap().into_path();
        ToolContextBuilder::new().with_cache_dir(cache_dir)
    }

    /// Instantiate a tool from a specification.
    ///
    /// If the tool is not already cached, it will be created, installed and
    /// cached.
    pub async fn instantiate(
        &self,
        spec: ToolSpec,
        channel_config: &ChannelConfig,
    ) -> Result<Tool, ToolCacheError> {
        let spec = match spec {
            ToolSpec::Isolated(isolated) => {
                if isolated.specs.is_empty() {
                    return Err(ToolCacheError::Install(miette!(
                        "No build match specs provided for '{}' command.",
                        isolated.command
                    )));
                }

                isolated
            }

            // I think we cannot bypass caching SystemTool as it is a wrapper around a spec command
            ToolSpec::System(system) => return Ok(Tool::System(SystemTool::new(system.command))),
        };

        let installed = self
            .cache
            .get_or_install_tool(
                spec,
                self,
                &self.cache_dir.join(CACHED_BUILD_TOOL_ENVS_DIR),
                channel_config,
            )
            .await
            .map_err(ToolCacheError::Install)?;

        // Return the installed tool as a non arc instance
        Ok(installed.as_ref().clone().into())
    }
}

impl ToolInstaller for ToolContext {
    /// Installed the tool in the isolated environment.
    async fn install(
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

        let build_api_version_nameless_spec = NamelessMatchSpec {
            version: Some(PIXI_BUILD_API_VERSION_SPEC.clone()),
            ..NamelessMatchSpec::default()
        };
        let build_api_version_spec = MatchSpec::from_nameless(
            build_api_version_nameless_spec,
            Some(PIXI_BUILD_API_VERSION_NAME.clone()),
        );

        let solved_records = Solver
            .solve(SolverTask {
                specs: spec.specs.clone(),
                virtual_packages,
                constraints: Vec::from([build_api_version_spec.clone()]),
                ..SolverTask::from_iter(&repodata)
            })
            .into_diagnostic()?;

        if !solved_records
            .records
            .iter()
            .any(|r| r.package_record.name == *PIXI_BUILD_API_VERSION_NAME)
        {
            return Err(miette::miette!(
                help = format!(
                    "Modify the requirements or contact the maintainers to ensure a dependency on `{}` is added.",
                    PIXI_BUILD_API_VERSION_NAME.as_normalized()
                ),
                "The environment for the build backend package does not depend on `{}`. Without this package pixi has no way of knowing the API to use to communicate with the backend.",
                PIXI_BUILD_API_VERSION_NAME.as_normalized()
            ));
        }

        let cache = EnvironmentHash::new(
            spec.command.clone(),
            spec.specs.clone(),
            channels.iter().map(|c| c.base_url.to_string()).collect(),
            self.platform,
        );

        // ensure that the cache directory exists
        fs_err::tokio::create_dir_all(&self.cache_dir)
            .await
            .into_diagnostic()?;

        let cached_dir = self
            .cache_dir
            .join(CACHED_BUILD_TOOL_ENVS_DIR)
            .join(cache.name());

        let prefix_guard = AsyncPrefixGuard::new(&cached_dir).await.into_diagnostic()?;

        let mut write_guard =
            await_in_progress("acquiring write lock on prefix", |_| prefix_guard.write())
                .await
                .into_diagnostic()?;

        // If the environment already exists, we can return early.
        if write_guard.is_ready() {
            tracing::info!("reusing existing environment in {}", cached_dir.display());

            write_guard.finish().await.into_diagnostic()?;

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
        write_guard.begin().await.into_diagnostic()?;

        // Install the environment
        Installer::new()
            .with_target_platform(self.platform)
            .with_download_client(self.client.clone())
            .with_package_cache(PackageCache::new(
                self.cache_dir
                    .join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
            ))
            .install(&cached_dir, solved_records.records)
            .await
            .into_diagnostic()?;

        // Get the activation scripts
        let activator =
            Activator::from_path(&cached_dir, ShellEnum::default(), self.platform).unwrap();

        let activation_scripts = activator
            .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
            .unwrap();

        write_guard.finish().await.into_diagnostic()?;

        Ok(IsolatedTool::new(
            spec.command.clone(),
            cached_dir,
            activation_scripts,
        ))
    }
}
