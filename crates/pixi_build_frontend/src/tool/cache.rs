use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
};

use dashmap::{DashMap, Entry};
use pixi_consts::consts::{CACHED_BUILD_ENVS_DIR, CONDA_REPODATA_CACHE_DIR};
use pixi_utils::reqwest::build_reqwest_clients;
use rattler::{install::Installer, package_cache::PackageCache};
use rattler_conda_types::{Channel, GenericVirtualPackage, MatchSpec, Platform};
use rattler_repodata_gateway::{ChannelConfig, Gateway};
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::{reqwest::Client, ClientWithMiddleware};

use super::IsolatedTool;
use crate::{
    tool::{SystemTool, Tool, ToolSpec},
    IsolatedToolSpec, SystemToolSpec,
};

#[derive(Hash)]
pub struct EnvironmentHash {
    pub command: String,
    pub specs: Vec<MatchSpec>,
    pub channels: Vec<String>,
}

impl EnvironmentHash {
    pub(crate) fn new(command: String, specs: Vec<MatchSpec>, channels: Vec<String>) -> Self {
        Self {
            command,
            specs,
            channels,
        }
    }

    /// Returns the name of the environment.
    pub(crate) fn name(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        let hash = hasher.finish();
        format!("{}-{:x}", &self.command, hash)
    }
}

#[derive(Default, Debug)]
pub struct ToolContext {
    pub gateway_config: ChannelConfig,
    pub client: ClientWithMiddleware,
    pub channels: Vec<Channel>,
}

impl ToolContext {
    pub fn new(
        gateway_config: ChannelConfig,
        client: ClientWithMiddleware,
        channels: Vec<Channel>,
    ) -> Self {
        Self {
            gateway_config,
            client,
            channels,
        }
    }
}

/// A [`ToolCache`] maintains a cache of environments for build tools.
///
/// This is useful to ensure that if we need to build multiple packages that use
/// the same tool, we can reuse their environments.
/// (nichita): it can also be seen as a way to create tools itself
#[derive(Default, Debug)]
pub struct ToolCache {
    pub cache: DashMap<CacheableToolSpec, CachedTool>,
    pub context: ToolContext,
}

#[derive(thiserror::Error, Debug)]
pub enum ToolCacheError {
    #[error("could not resolve '{}', {1}", .0.display())]
    Instantiate(PathBuf, which::Error),
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
            context: ToolContext::default(),
        }
    }

    pub fn with_context(self, context: ToolContext) -> Self {
        Self { context, ..self }
    }

    /// Instantiate a tool from a specification.
    ///
    /// If the tool is not already cached, it will be created and cached.
    pub async fn instantiate(&self, spec: ToolSpec) -> Result<Tool, ToolCacheError> {
        let spec = match spec {
            ToolSpec::Io(ipc) => return Ok(Tool::Io(ipc)),
            ToolSpec::Isolated(isolated) => CacheableToolSpec::Isolated(isolated),
            ToolSpec::System(system) => CacheableToolSpec::System(system),
        };

        let cache_entry = match self.cache.entry(spec.clone()) {
            Entry::Occupied(entry) => return Ok(entry.get().clone().into()),
            Entry::Vacant(entry) => entry,
        };

        let tool: CachedTool = match spec {
            CacheableToolSpec::Isolated(spec) => {
                // Don't isolate yet we are just pretending

                let cache_dir = pixi_config::get_cache_dir().unwrap();

                // collect existing dirs
                // and check if matchspec can satisfy the existing cache

                // construct the gateway
                // construct a new config
                let config = ChannelConfig {
                    default: self.context.gateway_config.default.clone(),
                    per_channel: self.context.gateway_config.per_channel.clone(),
                };

                let gateway = Gateway::builder()
                    .with_client(self.context.client.clone())
                    .with_cache_dir(cache_dir.join(CONDA_REPODATA_CACHE_DIR))
                    .with_channel_config(config)
                    .finish();

                let repodata = gateway
                    .query(
                        self.context.channels.clone(),
                        [Platform::current(), Platform::NoArch],
                        spec.specs.clone(),
                    )
                    .recursive(true)
                    .execute()
                    .await
                    .unwrap();

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
                    .unwrap();

                let cache = EnvironmentHash::new(
                    spec.command.clone(),
                    spec.specs,
                    self.context
                        .channels
                        .iter()
                        .map(|c| c.base_url().to_string())
                        .collect(),
                );

                let cached_dir = cache_dir.join(CACHED_BUILD_ENVS_DIR).join(cache.name());

                // Install the environment
                Installer::new()
                    .with_download_client(self.context.client.clone())
                    .with_package_cache(PackageCache::new(
                        cache_dir.join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
                    ))
                    .install(&cached_dir, solved_records)
                    .await
                    .unwrap();

                // get the activation scripts
                let activator =
                    Activator::from_path(&cached_dir, ShellEnum::default(), Platform::current())
                        .unwrap();

                let activation_scripts = activator
                    .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
                    .unwrap();

                IsolatedTool::new(spec.command, cached_dir, activation_scripts).into()
            }
            CacheableToolSpec::System(spec) => {
                // let exec = if spec.command.is_absolute() {
                //     spec.command.clone()
                // } else {
                //     which::which(&spec.command)
                //         .map_err(|e| ToolCacheError::Instantiate(spec.command.clone(), e))?
                // };
                SystemTool::new(spec.command.to_string_lossy().to_string()).into()
            }
        };

        cache_entry.insert(tool.clone());
        Ok(tool.into())
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use futures::channel;
    use pixi_config::Config;
    use rattler_conda_types::{ChannelConfig, MatchSpec, NamedChannelOrUrl, ParseStrictness};
    use reqwest_middleware::ClientWithMiddleware;

    use super::ToolCache;
    use crate::{
        tool::{IsolatedTool, SystemTool, Tool, ToolContext, ToolSpec},
        IsolatedToolSpec,
    };

    #[tokio::test]
    async fn test_tool_cache() {
        let cache = ToolCache::new();
        let mut config = Config::default();
        config.default_channels = vec![NamedChannelOrUrl::Name("conda-forge".to_string())];

        let auth_client = ClientWithMiddleware::default();
        let gateway_config = rattler_repodata_gateway::ChannelConfig::from(&config);
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());

        let channels = config
            .default_channels
            .iter()
            .cloned()
            .map(|c| c.into_channel(&channel_config).unwrap())
            .collect();

        let tool_context = ToolContext::new(gateway_config, auth_client, channels);

        let cache = cache.with_context(tool_context);

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("cowpy", ParseStrictness::Strict).unwrap()],
            command: "cowpy".into(),
        };

        let tool = cache
            .instantiate(ToolSpec::Isolated(tool_spec))
            .await
            .unwrap();

        let exec = tool.as_executable().unwrap();

        eprintln!("{:?}", exec);

        let output = exec.command().arg("hello").spawn().unwrap();

        eprintln!("{:?}", output);
    }
}
