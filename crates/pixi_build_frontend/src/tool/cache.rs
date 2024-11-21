use std::{fmt::Debug, hash::Hash, path::PathBuf};

use dashmap::{DashMap, Entry};
use miette::miette;
use pixi_consts::consts::CONDA_REPODATA_CACHE_DIR;
use rattler_conda_types::Channel;
use rattler_repodata_gateway::Gateway;
use reqwest_middleware::ClientWithMiddleware;

use super::IsolatedTool;
use crate::{
    tool::{SystemTool, Tool, ToolSpec},
    IsolatedToolSpec, SystemToolSpec,
};

pub struct ToolContextBuilder {
    gateway: Option<Gateway>,
    client: ClientWithMiddleware,
    channels: Vec<Channel>,
    cache_dir: PathBuf,
}

impl ToolContextBuilder {
    /// Create a new tool context builder.
    pub fn new(channels: Vec<Channel>) -> Self {
        Self {
            channels,
            gateway: None,
            client: ClientWithMiddleware::default(),
            cache_dir: pixi_config::get_cache_dir().expect("we should have a cache dir"),
        }
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

    /// Build the `ToolContext` using builder configuration.
    pub fn build(self) -> ToolContext {
        let gateway = self.gateway.unwrap_or_else(|| {
            Gateway::builder()
                .with_client(self.client.clone())
                .with_cache_dir(self.cache_dir.join(CONDA_REPODATA_CACHE_DIR))
                .finish()
        });

        ToolContext {
            channels: self.channels,
            cache_dir: self.cache_dir,
            client: self.client,
            gateway,
        }
    }
}

/// The tool context,
/// containing client, channels and gateway configuration
/// that will be used to resolve and install tools.
#[derive(Default, Clone)]
pub struct ToolContext {
    // Authentication client to use for fetching repodata.
    pub client: ClientWithMiddleware,
    /// The channels to use for resolving tools.
    pub channels: Vec<Channel>,
    // The cache directory to use for the tools.
    pub cache_dir: PathBuf,
    // The gateway to use for fetching repodata.
    pub gateway: Gateway,
}

impl Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("client", &self.client)
            .field("channels", &self.channels)
            .field("cache_dir", &self.cache_dir)
            .finish()
    }
}

impl ToolContext {
    /// Create a new tool context builder with the given channels.
    pub fn builder(channels: Vec<Channel>) -> ToolContextBuilder {
        ToolContextBuilder::new(channels)
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
    /// The context for the tools.
    /// It contains necessary details
    /// for the tools to be resolved and installed
    pub context: ToolContext,
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
            context: ToolContext::default(),
        }
    }

    /// Instantiate a tool from a specification.
    ///
    /// If the tool is not already cached, it will be created, installed and cached.
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
            CacheableToolSpec::Isolated(spec) => CachedTool::Isolated(if spec.specs.is_empty() {
                return Err(ToolCacheError::Install(miette!(
                    "No build specs provided for '{}' command.",
                    spec.command
                )));
            } else {
                spec.install(self.context.clone())
                    .await
                    .map_err(ToolCacheError::Install)?
            }),
            CacheableToolSpec::System(spec) => SystemTool::new(spec.command).into(),
        };

        cache_entry.insert(tool.clone());
        Ok(tool.into())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pixi_config::Config;
    use rattler_conda_types::{ChannelConfig, MatchSpec, NamedChannelOrUrl, ParseStrictness};
    use reqwest_middleware::ClientWithMiddleware;

    use super::ToolCache;
    use crate::{
        tool::{ToolContext, ToolSpec},
        IsolatedToolSpec,
    };

    #[tokio::test]
    async fn test_tool_cache() {
        let mut cache = ToolCache::new();
        let mut config = Config::default();
        config.default_channels = vec![NamedChannelOrUrl::Name("conda-forge".to_string())];

        let auth_client = ClientWithMiddleware::default();
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());

        let channels = config
            .default_channels
            .iter()
            .cloned()
            .map(|c| c.into_channel(&channel_config).unwrap())
            .collect();

        let tool_context = ToolContext::builder(channels)
            .with_client(auth_client.clone())
            .build();

        cache.context = tool_context;

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("cowpy", ParseStrictness::Strict).unwrap()],
            command: "cowpy".into(),
        };

        let tool = cache
            .instantiate(ToolSpec::Isolated(tool_spec))
            .await
            .unwrap();

        let exec = tool.as_executable().unwrap();

        exec.command().arg("hello").spawn().unwrap();
    }
}
