use std::{
    fmt::Debug,
    path::PathBuf,
    sync::{Arc, Weak},
};

use dashmap::{DashMap, Entry};
use rattler_conda_types::ChannelConfig;
use tokio::sync::broadcast;

use super::{installer::ToolInstaller, IsolatedTool};
use crate::IsolatedToolSpec;

/// A entity that is either pending or has been fetched.
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
        context: &impl ToolInstaller,
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
                            // currently installing the tool will not be able to store a value
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
        // other tasks will find a pending entry and will wait for the tool
        // to become available.
        //
        // Let's start by installing tool. If an error occurs we immediately return
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
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

    use pixi_config::Config;
    use rattler_conda_types::{ChannelConfig, MatchSpec, NamedChannelOrUrl, ParseStrictness};
    use reqwest_middleware::ClientWithMiddleware;
    use tokio::sync::{Barrier, Mutex};

    use crate::{
        tool::{
            installer::{ToolContext, ToolInstaller},
            IsolatedTool, ToolSpec,
        },
        IsolatedToolSpec,
    };

    /// A test installer that will count how many times a tool was installed.
    /// This is used to verify that we only install a tool once.
    #[derive(Default, Clone)]
    struct TestInstaller {
        count: Arc<Mutex<HashMap<IsolatedToolSpec, u8>>>,
    }

    impl ToolInstaller for TestInstaller {
        async fn install(
            &self,
            spec: &IsolatedToolSpec,
            _channel_config: &ChannelConfig,
        ) -> miette::Result<IsolatedTool> {
            let mut count = self.count.lock().await;
            let count = count.entry(spec.clone()).or_insert(0);
            *count += 1;

            let isolated_tool =
                IsolatedTool::new(spec.command.clone(), PathBuf::new(), HashMap::default());

            Ok(isolated_tool)
        }
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_installing_is_synced() {
        // This test verifies that we only install a tool once even if multiple tasks
        // request the same tool at the same time.

        let mut config = Config::default();
        config.default_channels = vec![NamedChannelOrUrl::Name("conda-forge".to_string())];

        let auth_client = ClientWithMiddleware::default();
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());

        let tool_context = Arc::new(
            ToolContext::builder()
                .with_client(auth_client.clone())
                .build(),
        );

        let tool_installer = TestInstaller::default();

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("cowpy", ParseStrictness::Strict).unwrap()],
            command: "cowpy".into(),
            channels: Vec::from([NamedChannelOrUrl::Name("conda-forge".to_string())]),
        };

        // Let's imitate that we have 4 requests to install a tool
        // we will use a barrier to ensure all tasks start at the same time.
        let num_tasks = 4;
        let barrier = Arc::new(Barrier::new(num_tasks));
        let mut handles = Vec::new();

        for _ in 0..num_tasks {
            let barrier = barrier.clone();

            let tool_context = tool_context.clone();

            let tool_installer = tool_installer.clone();

            let channel_config = channel_config.clone();
            let tool_spec = tool_spec.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                let tool = tool_context
                    .cache
                    .get_or_install_tool(tool_spec, &tool_installer, &channel_config)
                    .await;
                tool
            });

            handles.push(handle);
        }

        // Wait for all tasks to complete
        let tools = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|tool| tool.unwrap())
            .collect::<Vec<_>>();

        // verify that we dont have any errors
        let errors = tools.iter().filter(|tool| tool.is_err()).count();
        assert_eq!(errors, 0);

        // verify that only one was installed
        let lock = tool_installer.count.lock().await;
        let install_count = lock.get(&tool_spec).unwrap();
        assert_eq!(install_count, &1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_handle_a_failure() {
        // This test verifies that during the installation of a tool, if an error occurs
        // the tool is not cached and the next request will try to install the tool again.

        // A test installer that will fail on the first request.
        #[derive(Default, Clone)]
        struct TestInstaller {
            count: Arc<Mutex<HashMap<IsolatedToolSpec, u8>>>,
        }

        impl ToolInstaller for TestInstaller {
            async fn install(
                &self,
                spec: &IsolatedToolSpec,
                _channel_config: &ChannelConfig,
            ) -> miette::Result<IsolatedTool> {
                let mut count = self.count.lock().await;
                let count = count.entry(spec.clone()).or_insert(0);
                *count += 1;

                if count == &1 {
                    miette::bail!("error on first request");
                }

                let isolated_tool =
                    IsolatedTool::new(spec.command.clone(), PathBuf::new(), HashMap::default());
                Ok(isolated_tool)
            }
        }

        let mut config = Config::default();
        config.default_channels = vec![NamedChannelOrUrl::Name("conda-forge".to_string())];

        let auth_client = ClientWithMiddleware::default();
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::new());

        let tool_context = Arc::new(
            ToolContext::builder()
                .with_client(auth_client.clone())
                .build(),
        );

        let tool_installer = TestInstaller::default();

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("cowpy", ParseStrictness::Strict).unwrap()],
            command: "cowpy".into(),
            channels: Vec::from([NamedChannelOrUrl::Name("conda-forge".to_string())]),
        };

        // Let's imitate that we have 4 requests to install a tool
        // we will use a barrier to ensure all tasks start at the same time.
        let num_tasks = 4;
        let barrier = Arc::new(Barrier::new(num_tasks));
        let mut handles = Vec::new();

        for _ in 0..num_tasks {
            let barrier = barrier.clone();

            let tool_context = tool_context.clone();

            let tool_installer = tool_installer.clone();

            let channel_config = channel_config.clone();
            let tool_spec = tool_spec.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                let tool = tool_context
                    .cache
                    .get_or_install_tool(tool_spec, &tool_installer, &channel_config)
                    .await;
                tool
            });

            handles.push(handle);
        }

        // Wait for all tasks to complete
        let tools = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|tool| tool.unwrap())
            .collect::<Vec<_>>();

        // now we need to validate that exactly one install was errored out
        let errors = tools.iter().filter(|tool| tool.is_err()).count();
        assert_eq!(errors, 1);

        let lock = tool_installer.count.lock().await;
        let install_count = lock.get(&tool_spec).unwrap();
        assert_eq!(install_count, &2);
    }
}
