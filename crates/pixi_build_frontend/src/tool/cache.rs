use std::{
    fmt::Debug,
    path::PathBuf,
    sync::{Arc, Weak},
};

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
use tokio::sync::broadcast;

use super::IsolatedTool;
use crate::{
    tool::{SystemTool, Tool, ToolSpec},
    IsolatedToolSpec,
};

pub struct ToolContextBuilder {
    gateway: Option<Gateway>,
    client: ClientWithMiddleware,
    cache_dir: PathBuf,
    cache: ToolCache,
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
            gateway,
        }
    }
}

/// The tool context,
/// containing client, channels and gateway configuration
/// that will be used to resolve and install tools.
#[derive(Default)]
pub struct ToolContext {
    // Authentication client to use for fetching repodata.
    pub client: ClientWithMiddleware,
    // The cache directory to use for the tools.
    pub cache_dir: PathBuf,
    // The gateway to use for fetching repodata.
    pub gateway: Gateway,
    // The cache to use for the tools.
    pub cache: ToolCache,
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
            .get_or_install_tool(spec, self, channel_config)
            .await
            .map_err(ToolCacheError::Install)?;

        Ok(installed.into())
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
                [Platform::current(), Platform::NoArch],
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
            Activator::from_path(&cached_dir, ShellEnum::default(), Platform::current()).unwrap();

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

/// A record that is either pending or has been fetched.
#[derive(Clone)]
enum PendingOrFetched<T> {
    Pending(Weak<broadcast::Sender<T>>),
    Fetched(T),
}

/// A [`ToolCache`] maintains a cache of environments for isolated build tools.
///
/// This is useful to ensure that if we need to build multiple packages that use
/// the same tool, we can reuse their environments.
/// Implementation for request coalescing is inspired by:
/// * https://github.com/conda/rattler/blob/main/crates/rattler_repodata_gateway/src/gateway/mod.rs#L180
/// * https://github.com/prefix-dev/rip/blob/main/crates/rattler_installs_packages/src/wheel_builder/mod.rs#L39
#[derive(Default)]
pub struct ToolCache {
    /// The cache of tools.
    cache: DashMap<IsolatedToolSpec, PendingOrFetched<Arc<IsolatedTool>>>,
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

impl ToolCache {
    /// Construct a new tool cache.
    pub fn new() -> Self {
        Self {
            cache: DashMap::default(),
        }
    }

    pub async fn get_or_install_tool(
        &self,
        spec: IsolatedToolSpec,
        context: &ToolContext,
        channel_config: &ChannelConfig,
    ) -> miette::Result<Arc<IsolatedTool>> {
        let sender = match self.cache.entry(spec.clone()) {
            Entry::Vacant(entry) => {
                // Construct a sender so other tasks can subscribe
                let (sender, _) = broadcast::channel(1);
                let sender = Arc::new(sender);

                // modify the current entry to the pending entry.
                // this is an atomic operation
                // because who holds the entry holds mutable access.

                entry.insert(PendingOrFetched::Pending(Arc::downgrade(&sender)));

                sender
            }
            Entry::Occupied(mut entry) => {
                let tool = entry.get();
                match tool {
                    PendingOrFetched::Pending(sender) => {
                        let sender = sender.upgrade();
                        if let Some(sender) = sender {
                            // Create a receiver before we drop the entry. While we hold on to
                            // the entry we have exclusive access to it, this means the task
                            // currently fetching the subdir will not be able to store a value
                            // until we drop the entry.
                            // By creating the receiver here we ensure that we are subscribed
                            // before the other tasks sends a value over the channel.
                            let mut receiver = sender.subscribe();

                            // Explicitly drop the entry, so we don't block any other tasks.
                            drop(entry);

                            return match receiver.recv().await {
                                Ok(tool) => Ok(tool),
                                Err(_) => miette::bail!(
                                    "a coalesced tool {} request install failed",
                                    spec.command
                                ),
                            };
                        } else {
                            // Construct a sender so other tasks can subscribe
                            let (sender, _) = broadcast::channel(1);
                            let sender = Arc::new(sender);

                            // Modify the current entry to the pending entry, this is an atomic
                            // operation because who holds the entry holds mutable access.
                            entry.insert(PendingOrFetched::Pending(Arc::downgrade(&sender)));

                            sender
                        }
                    }
                    PendingOrFetched::Fetched(tool) => return Ok(tool.clone()),
                }
            }
        };

        // At this point we have exclusive write access to this specific entry. All
        // other tasks will find a pending entry and will wait for the records
        // to become available.
        //
        // Let's start by creating the subdir. If an error occurs we immediately return
        // the error. This will drop the sender and all other waiting tasks will
        // receive an error.
        let tool = Arc::new(context.install(&spec, channel_config).await?);

        // Store the fetched files in the entry.
        self.cache
            .insert(spec, PendingOrFetched::Fetched(tool.clone()));

        // Send the tool to all waiting tasks. We don't care if there are no
        // receivers, so we drop the error.
        let _ = sender.send(tool.clone());

        Ok(tool)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pixi_config::Config;
    use rattler_conda_types::{ChannelConfig, MatchSpec, NamedChannelOrUrl, ParseStrictness};
    use reqwest_middleware::ClientWithMiddleware;

    use crate::{
        tool::{ToolContext, ToolSpec},
        IsolatedToolSpec,
    };

    #[tokio::test]
    async fn test_tool_cache() {
        // let mut cache = ToolCache::new();
        let mut config = Config::default();
        config.default_channels = vec![NamedChannelOrUrl::Name("conda-forge".to_string())];

        let auth_client = ClientWithMiddleware::default();
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());

        let tool_context = ToolContext::builder()
            .with_client(auth_client.clone())
            .build();

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("cowpy", ParseStrictness::Strict).unwrap()],
            command: "cowpy".into(),
            channels: Vec::from([NamedChannelOrUrl::Name("conda-forge".to_string())]),
        };

        let tool = tool_context
            .instantiate(ToolSpec::Isolated(tool_spec), &channel_config)
            .await
            .unwrap();

        let exec = tool.as_executable().unwrap();

        exec.command().arg("hello").spawn().unwrap();
    }
}
