use std::{
    ffi::OsStr,
    fmt::Debug,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use dashmap::{DashMap, Entry};
use fs_err::tokio as tokio_fs;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_utils::AsyncPrefixGuard;
use rattler_conda_types::{ChannelConfig, Matches, Platform, PrefixRecord};
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use tokio::sync::broadcast;

use super::{IsolatedTool, installer::ToolInstaller};
use crate::IsolatedToolSpec;

/// A entity that is either pending or has been fetched.
#[derive(Clone)]
enum PendingOrFetched<T> {
    Pending(Weak<broadcast::Sender<T>>),
    Fetched(T),
}

/// A [`ToolCache`] maintains a cache of environments for build tools.
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

/// Finds the `PrefixRecord`s from `conda-meta` directory which starts with
/// `Matchspec` names.
pub(crate) async fn find_spec_records(
    conda_meta: &Path,
    name_to_match: Vec<String>,
) -> miette::Result<Option<Vec<PrefixRecord>>> {
    let mut read_dir = tokio_fs::read_dir(conda_meta).await.into_diagnostic()?;
    let mut records = Vec::new();

    // Set to keep track of which names have matching files
    let mut matched_names = std::collections::HashSet::new();

    while let Some(entry) = read_dir.next_entry().await.into_diagnostic()? {
        let path = entry.path();

        // Check if the entry is a file and has a .json extension
        if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("json") {
            if let Some(file_name) = path.file_name().and_then(OsStr::to_str) {
                // Check if the file name starts with any of the names in name_to_match
                for name in &name_to_match {
                    // Filename is in the form of: <name>-<version>-<build>
                    // this part is taken from ArchiveIdentifier
                    // https://github.com/conda/rattler/blob/b90daf5032e5c83ead9f9623576105ee08be837b/crates/rattler_conda_types/src/package/archive_identifier.rs#L11
                    let Some((_, _, filename)) = file_name.rsplitn(3, '-').next_tuple() else {
                        continue;
                    };

                    if name == filename {
                        matched_names.insert(name.clone());

                        let prefix_record = PrefixRecord::from_path(&path)
                            .into_diagnostic()
                            .wrap_err_with(|| {
                                format!("Couldn't parse JSON from {}", path.display())
                            })?;

                        records.push(prefix_record);
                    }
                }
            }
        }
    }

    // Check if all names in name_to_match were matched
    if matched_names.len() == name_to_match.len() {
        return Ok(Some(records));
    }

    Ok(None)
}

#[derive(thiserror::Error, Debug)]
pub enum ToolCacheError {
    #[error("could not resolve '{path}', {1}", path = .0.as_display())]
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
        cache_dir: &Path,
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
                            // // Drop the sender
                            drop(sender);

                            return match receiver.recv().await {
                                Ok(tool) => Ok(tool),
                                Err(err) => miette::bail!(
                                    "installing of {} tool failed. Reason: {err}",
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

        // Let's start by finding already existing matchspec
        let tool = match self.get_file_system_cached(&spec, cache_dir).await? {
            // Let's start by installing tool. If an error occurs we immediately return
            // the error. This will drop the sender and all other waiting tasks will
            // receive an error.
            // Installation happens outside the critical section
            None => {
                tracing::debug!("not found any existing environment for {:?}", spec.specs);
                context.install(&spec, channel_config).await?
            }

            Some(tool) => {
                tracing::debug!(
                    "reusing existing environment in {} for {:?}",
                    tool.prefix.display(),
                    spec.specs
                );
                tool
            }
        };

        let tool = Arc::new(tool);

        // Store the fetched files in the entry.
        self.cache
            .insert(spec, PendingOrFetched::Fetched(tool.clone()));

        // Send the tool to all waiting tasks. We don't care if there are no
        // receivers, so we drop the error.
        let _ = sender.send(tool.clone());

        Ok(tool)
    }

    /// Try to find already existing environment with the same tool spec
    /// in the cache directory.
    pub async fn get_file_system_cached(
        &self,
        spec: &IsolatedToolSpec,
        cache_dir: &Path,
    ) -> miette::Result<Option<IsolatedTool>> {
        // check if the cache directory exists
        if !cache_dir.exists() {
            return Ok(None);
        }

        let specs: Vec<String> = spec
            .specs
            .iter()
            .filter_map(|match_spec| match_spec.name.as_ref())
            .map(|name| name.as_normalized().to_string())
            .collect();

        if specs.len() != spec.specs.len() {
            return Ok(None);
        }

        // verify if we have a similar environment that match our matchspec
        // we need to load all prefix record from all folders in the cache
        // load all package records
        let mut entries = tokio_fs::read_dir(&cache_dir).await.into_diagnostic()?;
        let mut directories = Vec::new();

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            }
        }

        // let's find existing package records
        let mut records_of_records = Vec::new();

        for dir in directories.iter() {
            // Acquire a lock on the directory so we can safely read it.
            let prefix_guard = AsyncPrefixGuard::new(dir).await.into_diagnostic()?;
            let _prefix_guard = prefix_guard.write().await.into_diagnostic()?;

            let records = find_spec_records(&dir.join("conda-meta"), specs.clone()).await?;

            if let Some(records) = records {
                records_of_records.push((dir, records));
            }
        }

        // Find the first set of records where all specs in the manifest are present
        let matching_record = records_of_records.iter().find(|records| {
            spec.specs.iter().all(|spec| {
                records
                    .1
                    .iter()
                    .any(|record| spec.matches(&record.repodata_record.package_record))
            })
        });

        if let Some(records) = matching_record {
            // Get the activation scripts
            let activator =
                Activator::from_path(records.0, ShellEnum::default(), Platform::current()).unwrap();

            let activation_scripts = activator
                .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
                .unwrap();

            let cached_tool = IsolatedTool::new(
                spec.command.clone(),
                records.0.to_path_buf(),
                activation_scripts,
            );

            return Ok(Some(cached_tool));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

    use pixi_config::Config;
    use rattler_conda_types::{
        ChannelConfig, MatchSpec, NamedChannelOrUrl, ParseStrictness, Platform,
    };
    use reqwest_middleware::ClientWithMiddleware;
    use tokio::sync::{Barrier, Mutex, Semaphore};

    use crate::{
        IsolatedToolSpec,
        tool::{
            IsolatedTool, ToolSpec,
            cache::{ToolCache, find_spec_records},
            installer::{ToolContext, ToolInstaller},
        },
    };

    const BAT_META_JSON: &str = "bat-0.24.0-h3bba108_1.json";

    /// A test helper to create a temporary directory and write conda meta
    /// files. This is used to simulate already installed tools.
    struct CondaMetaWriter {
        pub tmp_dir: PathBuf,
    }

    impl CondaMetaWriter {
        async fn new() -> Self {
            let tempdir = tempfile::tempdir().unwrap();
            let tmp_dir = tempdir.path().to_path_buf();

            tokio::fs::create_dir_all(&tmp_dir).await.unwrap();
            Self { tmp_dir }
        }

        /// Write a meta-json file to the conda-meta directory.
        /// If `override_name` is provided, the file will be written with that
        /// name.
        async fn write_meta_json(
            &self,
            meta_json: &str,
            env_dir_name: &str,
            override_name: Option<&str>,
        ) {
            let bat_conda_meta = self.tmp_dir.join(env_dir_name).join("conda-meta");
            tokio::fs::create_dir_all(&bat_conda_meta).await.unwrap();

            let meta_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/conda-meta")
                .join(meta_json);
            // copy file and override the original name if necessary
            let name = override_name.unwrap_or(meta_json);
            tokio::fs::copy(meta_file, bat_conda_meta.join(name))
                .await
                .unwrap();
        }
    }

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

        let tool_context = ToolContext::for_tests()
            .with_platform(compatible_target_platform())
            .with_client(auth_client.clone())
            .with_gateway(config.gateway().with_client(auth_client).finish())
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

        tool.command()
            .arg("--version")
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
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
            ToolContext::for_tests()
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

                tool_context
                    .cache
                    .get_or_install_tool(
                        tool_spec,
                        &tool_installer,
                        &tool_context.cache_dir,
                        &channel_config,
                    )
                    .await
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
        // the tool is not cached and the next request will try to install the tool
        // again. A test installer that will fail on the first request.
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
            ToolContext::for_tests()
                .with_client(auth_client.clone())
                .build(),
        );

        let tool_installer = TestInstaller::default();

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("cowpy", ParseStrictness::Strict).unwrap()],
            command: "cowpy".into(),
            channels: Vec::from([NamedChannelOrUrl::Name("conda-forge".to_string())]),
        };

        let mut handles = Vec::new();

        // We need to test that failure of one task will not block other tasks
        // to execute.
        // To test it we want to synchronize the installation of the tool
        // in the following way
        // first task will fail, and set the semaphore to true
        // so other task can proceed to execute.
        // in this way we can verify that we handle a task failure correctly
        // and other tasks can proceed to install the tool.

        // It is is necessary to do it in this way because
        // without synchronization, all tasks will be blocked on the waiting stage
        // and failure of one task will be propagated to all other tasks.

        let semaphore = Arc::new(Semaphore::new(1));
        {
            let semaphore = semaphore.clone();

            let tool_context = tool_context.clone();
            let tool_installer = tool_installer.clone();

            let channel_config = channel_config.clone();
            let tool_spec = tool_spec.clone();

            let handle = tokio::spawn(async move {
                let _sem = semaphore.acquire().await.unwrap();

                tool_context
                    .cache
                    .get_or_install_tool(
                        tool_spec,
                        &tool_installer,
                        &tool_context.cache_dir,
                        &channel_config,
                    )
                    .await
            });
            handles.push(handle);
        }
        {
            let semaphore = semaphore.clone();

            let tool_context = tool_context.clone();
            let tool_installer = tool_installer.clone();

            let channel_config = channel_config.clone();
            let tool_spec = tool_spec.clone();

            let handle = tokio::spawn(async move {
                let _sem = semaphore.acquire().await.unwrap();
                tool_context
                    .cache
                    .get_or_install_tool(
                        tool_spec,
                        &tool_installer,
                        &tool_context.cache_dir,
                        &channel_config,
                    )
                    .await
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

    #[tokio::test]
    async fn test_can_find_from_filesystem() {
        let config = Config::for_tests();

        let tool_cache = ToolCache::new();

        let conda_meta_builder = CondaMetaWriter::new().await;

        conda_meta_builder
            .write_meta_json(BAT_META_JSON, "bat-somehash", None)
            .await;

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("bat", ParseStrictness::Strict).unwrap()],
            command: "bat".into(),
            channels: config.default_channels.clone(),
        };

        let tool = tool_cache
            .get_file_system_cached(&tool_spec, &conda_meta_builder.tmp_dir)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            tool.prefix
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            "bat-somehash"
        );
        assert_eq!(tool.command, "bat");
    }

    #[tokio::test]
    async fn test_missing_from_filesystem() {
        let config = Config::for_tests();

        let tool_cache = ToolCache::new();

        let conda_meta_builder = CondaMetaWriter::new().await;

        conda_meta_builder
            .write_meta_json(BAT_META_JSON, "bat-somehash", None)
            .await;

        let tool_spec = IsolatedToolSpec {
            specs: vec![MatchSpec::from_str("bat==1.0.0", ParseStrictness::Strict).unwrap()],
            command: "bat".into(),
            channels: config.default_channels.clone(),
        };

        let tool = tool_cache
            .get_file_system_cached(&tool_spec, &conda_meta_builder.tmp_dir)
            .await
            .unwrap();

        assert!(tool.is_none());
    }

    #[tokio::test]
    async fn test_find_specs() {
        let conda_meta_builder = CondaMetaWriter::new().await;

        conda_meta_builder
            .write_meta_json(BAT_META_JSON, "one-env", None)
            .await;

        // we have there bat and batt. We need to find only bat

        let records = find_spec_records(
            &conda_meta_builder
                .tmp_dir
                .join("one-env")
                .join("conda-meta"),
            vec!["bat".to_string()],
        )
        .await
        .unwrap()
        .unwrap();

        insta::assert_yaml_snapshot!(records);
    }

    #[tokio::test]
    async fn test_find_more_specs() {
        let conda_meta_builder = CondaMetaWriter::new().await;

        // write only one meta-json file, but ask for more specs
        conda_meta_builder
            .write_meta_json(BAT_META_JSON, "one-env", None)
            .await;

        // we have there bat and batt. We need to find only bat

        let records = find_spec_records(
            &conda_meta_builder
                .tmp_dir
                .join("one-env")
                .join("conda-meta"),
            vec!["bat".to_string(), "boltons".to_string()],
        )
        .await
        .unwrap();

        assert!(records.is_none());
    }

    #[tokio::test]
    async fn test_skip_wrong_json() {
        let conda_meta_builder = CondaMetaWriter::new().await;

        // verify that event when we have wrong json file, we will skip reading it.
        conda_meta_builder
            .write_meta_json(BAT_META_JSON, "one-env", Some("wrong.json"))
            .await;

        // we have there bat and batt. We need to find only bat

        let records = find_spec_records(
            &conda_meta_builder
                .tmp_dir
                .join("one-env")
                .join("conda-meta"),
            vec!["bat".to_string()],
        )
        .await
        .unwrap();
        assert!(records.is_none());
    }
}
