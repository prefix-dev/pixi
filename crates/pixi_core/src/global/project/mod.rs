use std::{
    ffi::OsStr,
    fmt::{Debug, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

pub use environment::EnvironmentName;
use fancy_display::FancyDisplay;
use fs::tokio as tokio_fs;
use fs_err as fs;
use futures::stream::StreamExt;
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use is_executable::IsExecutable;
use itertools::Itertools;
pub use manifest::{ExposedType, Manifest, Mapping};
use miette::{Context, IntoDiagnostic};
use once_cell::sync::OnceCell;
pub use parsed_manifest::ParsedManifest;
pub use parsed_manifest::{ExposedName, ParsedEnvironment};
use pixi_build_discovery::EnabledProtocols;
use pixi_command_dispatcher::{
    BuildEnvironment, CommandDispatcher, InstallPixiEnvironmentSpec, Limits, PixiEnvironmentSpec,
};
use pixi_config::{Config, RunPostLinkScripts, default_channel_config, pixi_home};
use pixi_consts::consts::{self};
use pixi_manifest::PrioritizedChannel;
use pixi_progress::global_multi_progress;
use pixi_reporters::TopLevelProgress;
use pixi_spec_containers::DependencyMap;
use pixi_utils::prefix::{Executable, Prefix};
use pixi_utils::rlimit::try_increase_rlimit_to_sensible;
use pixi_utils::{executable_from_path, reqwest::build_reqwest_clients};
use rattler_conda_types::{
    ChannelConfig, GenericVirtualPackage, MatchSpec, PackageName, Platform, PrefixRecord,
    menuinst::MenuMode,
};
use rattler_repodata_gateway::Gateway;
use std::collections::HashSet;
// Removed unused rattler_solve imports
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackage, VirtualPackageOverrides,
};
use reqwest_middleware::ClientWithMiddleware;
use tokio::sync::Semaphore;
use toml_edit::DocumentMut;

use self::trampoline::{Configuration, ConfigurationParseError, Trampoline};
use super::{
    BinDir, EnvRoot, StateChange, StateChanges,
    common::{EnvironmentUpdate, get_install_changes, shortcuts_sync_status},
    install::find_binary_by_name,
    trampoline::{self, GlobalExecutable},
};
use crate::{
    global::{
        EnvDir,
        common::{
            channel_url_to_prioritized_channel, expose_scripts_sync_status, find_package_records,
        },
        find_executables, find_executables_for_many_records,
        install::{create_executable_trampolines, script_exec_mapping},
        project::environment::environment_specs_in_sync,
    },
    repodata::Repodata,
};

mod environment;
mod global_spec;
mod manifest;
mod parsed_manifest;
pub use global_spec::{FromMatchSpecError, GlobalSpec, NamedGlobalSpec};
use pixi_build_frontend::BackendOverride;

pub(crate) const MANIFESTS_DIR: &str = "manifests";

/// The pixi global project, this main struct to interact with the pixi global
/// project. This struct holds the `Manifest` and has functions to modify
/// or request information from it. This allows in the future to have multiple
/// manifests linked to a pixi global project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    pub root: PathBuf,
    /// The manifest for the project
    pub manifest: Manifest,
    /// The global configuration as loaded from the config file(s)
    config: Config,
    /// Root directory of the global environments
    pub(crate) env_root: EnvRoot,
    /// Binary directory
    pub(crate) bin_dir: BinDir,
    /// Reqwest client shared for this project.
    /// This is wrapped in a `OnceCell` to allow for lazy initialization.
    // TODO: once https://github.com/rust-lang/rust/issues/109737 is stabilized, switch to OnceLock
    client: OnceCell<(reqwest::Client, ClientWithMiddleware)>,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceCell` to allow for lazy initialization.
    // TODO: once https://github.com/rust-lang/rust/issues/109737 is stabilized, switch to OnceLock
    repodata_gateway: OnceCell<Gateway>,
    /// The concurrent request semaphore
    concurrent_downloads_semaphore: OnceCell<Arc<Semaphore>>,
    /// The command dispatcher for solving environments
    /// This is wrapped in a `OnceCell` to allow for lazy initialization.
    command_dispatcher: OnceCell<CommandDispatcher>,
}

impl Debug for Project {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Global Project")
            .field("root", &self.root)
            .field("manifest", &self.manifest)
            .finish()
    }
}

/// Intermediate struct to store all the binaries that are exposed.
#[derive(Debug)]
struct ExposedData {
    env_name: EnvironmentName,
    platform: Option<Platform>,
    channels: Vec<PrioritizedChannel>,
    package: PackageName,
    exposed: ExposedName,
    executable_name: String,
}

impl ExposedData {
    /// Constructs an `ExposedData` instance from a exposed `script` or
    /// `trampoline` path.
    ///
    /// This function extracts metadata from the exposed script path, including
    /// the environment name, platform, channel, and package information, by
    /// reading the associated `conda-meta` directory.
    /// or it looks into the trampoline manifest to extract the metadata.
    pub async fn from_exposed_path(
        bin: &GlobalExecutable,
        env_root: &EnvRoot,
        channel_config: &ChannelConfig,
    ) -> miette::Result<Self> {
        let exposed = bin.exposed_name();
        let executable_path = bin.executable().await?;

        let executable = executable_from_path(&executable_path);
        let env_path = determine_env_path(&executable_path, env_root.path())?;
        let env_name = env_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| {
                miette::miette!(
                    "executable path's grandparent '{}' has no file name",
                    executable_path.display()
                )
            })
            .and_then(|env| EnvironmentName::from_str(env).into_diagnostic())?;

        let conda_meta = env_path.join(consts::CONDA_META_DIR);
        let env_dir = EnvDir::from_env_root(env_root.clone(), &env_name).await?;
        let prefix = Prefix::new(env_dir.path());

        let (platform, channel, package) =
            package_from_conda_meta(&conda_meta, &executable, &prefix, channel_config).await?;

        let mut channels = vec![channel];

        // Find all channels used to create the environment
        let all_channels = prefix
            .find_installed_packages()?
            .iter()
            .map(|prefix_record| prefix_record.repodata_record.channel.clone())
            .collect::<HashSet<_>>();

        for channel in all_channels.into_iter().flatten() {
            tracing::debug!("Channel: {} found in environment: {}", channel, env_name);
            channels.push(channel_url_to_prioritized_channel(
                &channel,
                channel_config,
            )?);
        }

        Ok(ExposedData {
            env_name,
            platform,
            channels,
            package,
            executable_name: executable,
            exposed,
        })
    }
}

fn determine_env_path(executable_path: &Path, env_root: &Path) -> miette::Result<PathBuf> {
    let mut current_path = executable_path;

    while let Some(parent) = current_path.parent() {
        if parent == env_root {
            return Ok(current_path.to_owned());
        }
        current_path = parent;
    }

    miette::bail!(
        "Couldn't determine environment path: no parent of '{}' has '{}' as its direct parent",
        executable_path.display(),
        env_root.display()
    )
}

/// Converts a `PrefixRecord` into package metadata, including platform,
/// channel, and package name.
fn convert_record_to_metadata(
    prefix_record: &PrefixRecord,
    channel_config: &ChannelConfig,
) -> miette::Result<(Option<Platform>, PrioritizedChannel, PackageName)> {
    let platform = match Platform::from_str(&prefix_record.repodata_record.package_record.subdir) {
        Ok(Platform::NoArch) => None,
        Ok(platform) if platform == Platform::current() => None,
        Err(_) => None,
        Ok(p) => Some(p),
    };

    let package_name = prefix_record.repodata_record.package_record.name.clone();

    let Some(channel_str) = prefix_record.repodata_record.channel.as_deref() else {
        miette::bail!(
            "missing channel in prefix record for {}",
            package_name.as_source()
        )
    };

    let channel = channel_url_to_prioritized_channel(channel_str, channel_config)?;

    Ok((platform, channel, package_name))
}

/// Extracts package metadata from the `conda-meta` directory for a given
/// executable.
///
/// This function reads the `conda-meta` directory to find the package metadata
/// associated with the specified executable. It returns the platform, channel,
/// and package name of the executable.
async fn package_from_conda_meta(
    conda_meta: &Path,
    executable: &str,
    prefix: &Prefix,
    channel_config: &ChannelConfig,
) -> miette::Result<(Option<Platform>, PrioritizedChannel, PackageName)> {
    let records = find_package_records(conda_meta).await?;

    for prefix_record in records {
        if find_executables(prefix, &prefix_record)
            .iter()
            .any(|exe_path| executable_from_path(exe_path) == executable)
        {
            return convert_record_to_metadata(&prefix_record, channel_config);
        }
    }

    miette::bail!("Couldn't find {executable} in {}", conda_meta.display())
}

impl Project {
    /// Constructs a new instance from an internal manifest representation
    pub(crate) fn from_manifest(manifest: Manifest, env_root: EnvRoot, bin_dir: BinDir) -> Self {
        let root = manifest
            .path
            .parent()
            .expect("manifest path should always have a parent")
            .to_owned();

        let config = Config::load(&root);

        let client = OnceCell::new();
        let repodata_gateway = OnceCell::new();
        Self {
            root,
            manifest,
            config,
            env_root,
            bin_dir,
            client,
            repodata_gateway,
            concurrent_downloads_semaphore: OnceCell::new(),
            command_dispatcher: OnceCell::new(),
        }
    }

    /// Constructs a project from a manifest.
    pub(crate) fn from_str(
        manifest_path: &Path,
        content: &str,
        env_root: EnvRoot,
        bin_dir: BinDir,
    ) -> miette::Result<Self> {
        let manifest = Manifest::from_str(manifest_path, content)?;
        Ok(Self::from_manifest(manifest, env_root, bin_dir))
    }

    /// Discovers the project manifest file in path at
    /// `~/.pixi/manifests/pixi-global.toml`. If the manifest doesn't exist
    /// yet, and the function will try to create one from the existing
    /// installation. If that one fails, an empty one will be created.
    pub async fn discover_or_create() -> miette::Result<Self> {
        let manifest_dir = Self::manifest_dir()?;
        let manifest_path = Self::default_manifest_path()?;
        // Prompt user if the manifest is empty and the user wants to create one

        let bin_dir = BinDir::from_env().await?;
        let env_root = EnvRoot::from_env().await?;

        if !manifest_path.exists() {
            tracing::debug!(
                "Global manifest {} doesn't exist yet. Creating a new one.",
                manifest_path.display()
            );
            tokio_fs::create_dir_all(&manifest_dir)
                .await
                .into_diagnostic()?;

            if !env_root.directories().await?.is_empty() {
                tracing::debug!(
                    "Existing installation found. Creating global manifest from that information."
                );
                return Self::try_from_existing_installation(&manifest_path, env_root, bin_dir)
                    .await
                    .wrap_err_with(
                        || "Failed to create global manifest from existing installation",
                    );
            } else {
                tracing::debug!("Create an empty global manifest.");
                tokio_fs::File::create(&manifest_path)
                    .await
                    .into_diagnostic()?;
            }
        }

        Self::from_path(&manifest_path, env_root, bin_dir)
    }

    async fn try_from_existing_installation(
        manifest_path: &Path,
        env_root: EnvRoot,
        bin_dir: BinDir,
    ) -> miette::Result<Self> {
        let config = Config::load(env_root.path());

        let exposed_binaries: Vec<ExposedData> = bin_dir
            .executables()
            .await?
            .into_iter()
            .map(|bin| {
                let env_root = env_root.clone();
                let config = config.clone();
                async move {
                    ExposedData::from_exposed_path(&bin, &env_root, config.global_channel_config())
                        .await
                }
            })
            .collect::<futures::stream::FuturesOrdered<_>>()
            .filter_map(|result| async {
                match result {
                    Ok(data) => Some(data),
                    Err(e) => {
                        tracing::warn!("{e}");
                        None
                    }
                }
            })
            .collect()
            .await;

        let parsed_manifest = ParsedManifest::from(exposed_binaries);
        let toml_pretty = toml_edit::ser::to_string_pretty(&parsed_manifest).into_diagnostic()?;
        let mut document: DocumentMut = toml_pretty.parse().into_diagnostic()?;

        // Ensure that the manifest uses inline tables for "dependencies" and "exposed"
        if let Some(envs) = document
            .get_mut("envs")
            .and_then(|item| item.as_table_mut())
        {
            for (_, env_table) in envs.iter_mut() {
                let Some(env_table) = env_table.as_table_mut() else {
                    continue;
                };

                for entry in ["dependencies", "exposed"] {
                    if let Some(table) = env_table.get(entry).and_then(|item| item.as_table()) {
                        env_table
                            .insert(entry, toml_edit::value(table.clone().into_inline_table()));
                    }
                }
            }
        }
        let toml = document.to_string();
        tokio_fs::write(&manifest_path, &toml)
            .await
            .into_diagnostic()?;
        Self::from_str(manifest_path, &toml, env_root, bin_dir)
    }

    /// Get default dir for the pixi global manifest
    pub fn manifest_dir() -> miette::Result<PathBuf> {
        // Potential directories, with the highest priority coming first
        let potential_dirs = [
            pixi_home(),
            dirs::config_dir().map(|dir| dir.join(consts::CONFIG_DIR)),
        ]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(MANIFESTS_DIR))
        .collect_vec();

        // First, check if a `pixi-global.toml` already exists
        for dir in &potential_dirs {
            if dir.join(consts::GLOBAL_MANIFEST_DEFAULT_NAME).is_file() {
                return Ok(dir.clone());
            }
        }

        // If not, return the first option
        potential_dirs
            .first()
            .cloned()
            .ok_or_else(|| miette::miette!("Couldn't obtain global manifest directory"))
    }

    /// Get the default path to the global manifest file
    pub fn default_manifest_path() -> miette::Result<PathBuf> {
        Self::manifest_dir().map(|dir| dir.join(consts::GLOBAL_MANIFEST_DEFAULT_NAME))
    }

    /// Loads a project from manifest file.
    pub(crate) fn from_path(
        manifest_path: &Path,
        env_root: EnvRoot,
        bin_dir: BinDir,
    ) -> miette::Result<Self> {
        let manifest = Manifest::from_path(manifest_path)?;
        Ok(Project::from_manifest(manifest, env_root, bin_dir))
    }

    /// Merge config with existing config project
    pub fn with_cli_config<C>(mut self, config: C) -> Self
    where
        C: Into<Config>,
    {
        self.config = self.config.merge_config(config.into());
        self
    }

    /// Returns the environments in this project.
    pub fn environments(&self) -> &IndexMap<EnvironmentName, ParsedEnvironment> {
        &self.manifest.parsed.envs
    }

    /// Return the environment with the given name.
    pub fn environment(&self, name: &EnvironmentName) -> Option<&ParsedEnvironment> {
        self.manifest.parsed.envs.get(name)
    }

    /// Returns the EnvDir with the environment name.
    pub async fn environment_dir(&self, name: &EnvironmentName) -> miette::Result<EnvDir> {
        EnvDir::from_env_root(self.env_root.clone(), name).await
    }

    /// Returns the prefix of the environment with the given name.
    pub async fn environment_prefix(&self, env_name: &EnvironmentName) -> miette::Result<Prefix> {
        Ok(Prefix::new(self.environment_dir(env_name).await?.path()))
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> miette::Result<&ClientWithMiddleware> {
        Ok(&self.client_and_authenticated_client()?.1)
    }

    fn client_and_authenticated_client(
        &self,
    ) -> miette::Result<&(reqwest::Client, ClientWithMiddleware)> {
        self.client
            .get_or_try_init(|| build_reqwest_clients(Some(&self.config), None))
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn global_channel_config(&self) -> &ChannelConfig {
        self.config.global_channel_config()
    }

    /// Check if the platform matches the current platform (OS)
    /// We only need to detect virtual packages if the platform is the current one.
    /// Otherwise, we use an empty list
    pub(crate) fn virtual_packages_for(
        platform: &Platform,
    ) -> Result<Vec<GenericVirtualPackage>, DetectVirtualPackageError> {
        if platform
            .only_platform()
            .map(|p| p == Platform::current().only_platform().unwrap_or(""))
            .unwrap_or(false)
        {
            Ok(VirtualPackage::detect(&VirtualPackageOverrides::default())?
                .iter()
                .cloned()
                .map(GenericVirtualPackage::from)
                .collect())
        } else {
            Ok(vec![])
        }
    }

    pub async fn install_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<EnvironmentUpdate> {
        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;
        let channels = environment
            .channels()
            .into_iter()
            .map(|channel| {
                channel
                    .clone()
                    .into_channel(self.config.global_channel_config())
            })
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()?;

        let platform = environment.platform.unwrap_or_else(Platform::current);

        // Convert dependency specs to binary specs for CommandDispatcher
        let mut pixi_specs = DependencyMap::default();
        let mut dependencies_names = Vec::new();

        for (name, spec) in &environment.dependencies.specs {
            pixi_specs.insert(name.clone(), spec.clone());
            dependencies_names.push(name.clone());
        }

        let command_dispatcher = self.command_dispatcher()?;

        let channels = channels
            .into_iter()
            .map(|channel| channel.base_url.clone())
            .collect::<Vec<_>>();

        let build_environment = BuildEnvironment::simple(
            platform,
            Self::virtual_packages_for(&platform).into_diagnostic()?,
        );
        // Create solve spec
        let solve_spec = PixiEnvironmentSpec {
            name: Some(env_name.to_string()),
            dependencies: pixi_specs,
            build_environment: build_environment.clone(),
            channels: channels.clone(),
            channel_config: self.config.global_channel_config().clone(),
            ..Default::default()
        };

        // Solve using CommandDispatcher
        let pixi_records = command_dispatcher
            .solve_pixi_environment(solve_spec)
            .await?;

        // Move this to a separate function to avoid code duplication
        try_increase_rlimit_to_sensible();

        let prefix = self.environment_prefix(env_name).await?;

        let result = command_dispatcher
            .install_pixi_environment(InstallPixiEnvironmentSpec {
                name: env_name.to_string(),
                records: pixi_records,
                prefix: rattler_conda_types::prefix::Prefix::create(prefix.root())
                    .into_diagnostic()?,
                build_environment,
                channels,
                channel_config: self.config.global_channel_config().clone(),
                enabled_protocols: EnabledProtocols::default(),
                installed: None,
                force_reinstall: Default::default(),
                variants: None,
            })
            .await?;

        command_dispatcher.clear_reporter().await;

        let install_changes = get_install_changes(result.transaction);
        Ok(EnvironmentUpdate::new(install_changes, dependencies_names))
    }

    /// Remove an environment from the manifest and the global installation.
    pub async fn remove_environment(
        &mut self,
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        // Check if the environment exists in the manifest first, before creating any
        // directories
        if !self.manifest.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist.", env_name.fancy_display());
        }

        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;
        let mut state_changes = StateChanges::new_with_env(env_name.clone());

        // Remove all shortcuts, using the information still available in the
        // environment
        state_changes |= self.remove_shortcuts(env_name).await?;

        // Remove the environment from the manifest, if it exists, otherwise ignore
        // error.
        self.manifest.remove_environment(env_name)?;

        // Remove the environment
        tokio_fs::remove_dir_all(env_dir.path())
            .await
            .into_diagnostic()?;

        // Get all removable binaries related to the environment
        let (to_remove, _to_add) =
            expose_scripts_sync_status(&self.bin_dir, &env_dir, &IndexSet::new()).await?;

        // Remove all removable binaries
        for binary_path in to_remove {
            binary_path.remove().await?;
            state_changes.insert_change(
                env_name,
                StateChange::RemovedExposed(binary_path.exposed_name()),
            );
        }

        #[cfg(unix)] // Completions are only supported on unix-like systems
        {
            // Prune old completions
            let completions_dir = super::completions::CompletionsDir::from_env().await?;
            completions_dir.prune_old_completions()?;
        }

        state_changes.insert_change(env_name, StateChange::RemovedEnvironment);

        Ok(state_changes)
    }

    /// Find all binaries related to the environment and remove those that are
    /// not listed as exposed.
    pub async fn prune_exposed(&self, env_name: &EnvironmentName) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();
        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;
        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;

        // Get all removable binaries related to the environment
        let (to_remove, _to_add) =
            expose_scripts_sync_status(&self.bin_dir, &env_dir, &environment.exposed).await?;

        // Remove all removable binaries
        for exposed_path in to_remove {
            state_changes.insert_change(
                env_name,
                StateChange::RemovedExposed(exposed_path.exposed_name()),
            );
            exposed_path.remove().await?;
        }

        Ok(state_changes)
    }

    /// Gets all installed executables of a specific environment.
    pub async fn executables_of_all_dependencies(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<Vec<Executable>> {
        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;
        let prefix = Prefix::new(env_dir.path());

        let prefix_records = &prefix.find_installed_packages()?;

        let all_executables = find_executables_for_many_records(&prefix, prefix_records);

        Ok(all_executables)
    }

    /// Get installed executables of direct dependencies of a specific
    /// environment.
    pub async fn executables_of_direct_dependencies(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<IndexMap<PackageName, Vec<Executable>>> {
        let parsed_env = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;

        let package_names: Vec<_> = parsed_env.dependencies.specs.keys().cloned().collect();

        let mut executables_for_package = IndexMap::new();

        for package_name in &package_names {
            let prefix = self.environment_prefix(env_name).await?;
            let prefix_package = prefix.find_designated_package(package_name).await?;
            let mut package_executables = prefix.find_executables(&[prefix_package]);

            // Sometimes the package don't ship executables on their own.
            // We need to search for it in different packages.
            if !package_executables
                .iter()
                .any(|executable| executable.name.as_str() == package_name.as_normalized())
            {
                if let Some(exec) = find_binary_by_name(&prefix, package_name).await? {
                    package_executables.push(exec);
                }
            }

            executables_for_package.insert(package_name.clone(), package_executables);
        }
        Ok(executables_for_package)
    }

    /// Sync the `exposed` field in manifest based on the executables in the
    /// environment and the expose type. Expose type can be either:
    /// * If the user initially chooses to auto-expose everything, we will add
    ///   new binaries that are not exposed in the `exposed` field.
    ///
    /// * If the use chose to expose only a subset of binaries, we will remove
    ///   the binaries that are not anymore present in the environment and will
    ///   not expose the new ones
    pub async fn sync_exposed_names(
        &mut self,
        env_name: &EnvironmentName,
        expose_type: ExposedType,
    ) -> miette::Result<()> {
        // Get env executables
        let execs_all = self.executables_of_all_dependencies(env_name).await?;

        // Get the parsed environment
        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;

        // Find the exposed names that are no longer there and remove them
        let to_remove = environment
            .exposed
            .iter()
            .filter_map(|mapping| {
                // If the executable isn't requested, remove the mapping
                // Use file name of executable relname here for custom exposed path.
                // `exposed = {dotnet = 'dotnet\dotnet' }`, file_name will be `dotnet`, eg.
                let executable_file_name = PathBuf::from(mapping.executable_relname())
                    .file_name()?
                    .to_string_lossy()
                    .to_string();
                if execs_all.iter().all(|executable| {
                    executable_from_path(&executable.path) != executable_file_name
                }) {
                    Some(mapping.exposed_name().clone())
                } else {
                    None
                }
            })
            .collect_vec();

        // Removed the removable exposed names from the manifest
        for exposed_name in &to_remove {
            self.manifest.remove_exposed_name(env_name, exposed_name)?;
        }

        let execs_direct_deps = self.executables_of_direct_dependencies(env_name).await?;

        match expose_type {
            ExposedType::All => {
                // Add new binaries that are not yet exposed
                let executable_names = execs_direct_deps
                    .into_iter()
                    .flat_map(|(_, executables)| executables)
                    .map(|executable| executable.name);
                for executable_name in executable_names {
                    let mapping = Mapping::new(
                        ExposedName::from_str(&executable_name)?,
                        executable_name.to_string(),
                    );
                    self.manifest.add_exposed_mapping(env_name, &mapping)?;
                }
            }
            ExposedType::Nothing => {}
            ExposedType::Ignore(ignore) => {
                // Add new binaries that are not yet exposed and that don't come from one of the
                // packages we ignore
                let executable_names = execs_direct_deps
                    .into_iter()
                    .filter_map(|(package_name, executable)| {
                        if ignore.contains(&package_name) {
                            None
                        } else {
                            Some(executable)
                        }
                    })
                    .flatten()
                    .map(|executable| executable.name);

                for executable_name in executable_names {
                    let mapping = Mapping::new(
                        ExposedName::from_str(&executable_name)?,
                        executable_name.to_string(),
                    );
                    self.manifest.add_exposed_mapping(env_name, &mapping)?;
                }
            }
            ExposedType::Mappings(mapping) => {
                // Expose only the requested binaries
                for mapping in mapping {
                    self.manifest.add_exposed_mapping(env_name, &mapping)?;
                }
            }
        }

        Ok(())
    }

    /// Check if the environment is in sync with the manifest
    ///
    /// Validated the specs in the installed environment.
    /// And verifies only and all required exposed binaries are in the bin dir.
    pub async fn environment_in_sync(&self, env_name: &EnvironmentName) -> miette::Result<bool> {
        self.environment_in_sync_internal(env_name, false).await
    }

    /// Internal method to check environment sync with update operation control
    pub async fn environment_in_sync_internal(
        &self,
        env_name: &EnvironmentName,
        is_update_operation: bool,
    ) -> miette::Result<bool> {
        let environment = self.environment(env_name).ok_or(miette::miette!(
            "Environment {} not found in manifest.",
            env_name.fancy_display()
        ))?;

        // Split the environment into source and binary requirements
        let (source_specs, binary_specs) = environment.split_into_source_and_binary_requirements();
        // Convert binary specs to MatchSpec, these can be matched against the prefix directly
        let specs = binary_specs
            .into_iter()
            .map(|(name, spec)| {
                let match_spec = MatchSpec::from_nameless(
                    spec.clone()
                        .try_into_nameless_match_spec(&default_channel_config())
                        .into_diagnostic()?
                        .ok_or_else(|| {
                            miette::miette!("Couldn't convert {spec:?} to nameless match spec.")
                        })?,
                    Some(name.clone()),
                );
                Ok(match_spec)
            })
            .collect::<Result<IndexSet<MatchSpec>, miette::Report>>()?;

        let source_package_names = source_specs.specs.keys().cloned().collect::<HashSet<_>>();

        // For update operations, always consider environments with source dependencies as out of sync
        if is_update_operation && !source_package_names.is_empty() {
            tracing::debug!(
                "Update operation: Environment {} has source dependencies, considering out of sync",
                env_name.fancy_display()
            );
            tracing::debug!(
                "Environment out of sync because update operation has source dependencies"
            );
            return Ok(false);
        }

        let env_dir =
            EnvDir::from_path(self.env_root.clone().path().join(env_name.clone().as_str()));

        let prefix = self.environment_prefix(env_name).await?;
        let prefix_records = prefix.find_installed_packages()?;
        let specs_in_sync = environment_specs_in_sync(
            &prefix_records,
            &specs,
            &source_package_names,
            environment.platform,
        )
        .await?;
        if !specs_in_sync {
            tracing::debug!("Environment out of sync because package specifications don't match");
            return Ok(false);
        }

        tracing::debug!("Verify that the binaries are in sync with the environment");
        let (exec_to_remove, exec_to_add) =
            expose_scripts_sync_status(&self.bin_dir, &env_dir, &environment.exposed).await?;
        if !exec_to_remove.is_empty() || !exec_to_add.is_empty() {
            tracing::debug!(
                "Environment {} binaries are not in sync: to_remove: {:?}, to_add: {:?}",
                env_name.fancy_display(),
                exec_to_remove,
                exec_to_add
            );
            tracing::debug!("Environment out of sync because binaries need to be updated");
            return Ok(false);
        }

        tracing::debug!("Verify that the shortcuts are in sync with the environment");
        let shortcuts = environment.shortcuts.clone().unwrap_or_default();
        let (shortcuts_to_remove, shortcuts_to_add) =
            shortcuts_sync_status(shortcuts, prefix_records, prefix.root())?;
        if !shortcuts_to_remove.is_empty() || !shortcuts_to_add.is_empty() {
            tracing::debug!(
                "Environment {} shortcuts are not in sync: to_remove: {}, to_add: {}",
                env_name.fancy_display(),
                shortcuts_to_remove
                    .iter()
                    .map(|s| s.repodata_record.package_record.name.as_normalized())
                    .join(", "),
                shortcuts_to_add
                    .iter()
                    .map(|s| s.repodata_record.package_record.name.as_normalized())
                    .join(", ")
            );
            tracing::debug!("Environment out of sync because shortcuts need to be updated");
            return Ok(false);
        }

        tracing::debug!("Environment is in sync");
        Ok(true)
    }

    /// Check if all environments are in sync with the manifest
    pub async fn environments_in_sync(&self) -> miette::Result<bool> {
        let mut in_sync = true;
        for (env_name, _parsed_environment) in self.environments() {
            if !self.environment_in_sync(env_name).await? {
                tracing::debug!(
                    "Environment {} not up to date with the manifest",
                    env_name.fancy_display()
                );
                in_sync = false;
            }
        }
        Ok(in_sync)
    }

    /// Expose executables from the environment to the global bin directory.
    ///
    /// This function will first remove all binaries that are not listed as
    /// exposed. It will then create an activation script for the shell and
    /// create the scripts.
    pub async fn expose_executables_from_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();

        // First clean up binaries that are not listed as exposed
        state_changes |= self.prune_exposed(env_name).await?;

        let all_executables = self.executables_of_all_dependencies(env_name).await?;

        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;
        let prefix = Prefix::new(env_dir.path());

        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;

        let exposed: HashSet<&str> = environment
            .exposed
            .iter()
            .map(|map| map.executable_name())
            .collect();

        let exposed_executables: Vec<_> = all_executables
            .iter()
            .filter(|executable| exposed.contains(executable.name.as_str()))
            .cloned()
            .collect();

        let script_mapping = environment
            .exposed
            .iter()
            .map(|mapping| {
                script_exec_mapping(
                    mapping.exposed_name(),
                    mapping.executable_name(),
                    exposed_executables.iter(),
                    &self.bin_dir,
                    &env_dir,
                )
            })
            .collect::<miette::Result<Vec<_>>>()
            .wrap_err(format!(
                "Failed to add executables for environment: {}",
                env_name
            ))?;

        tracing::debug!(
            "Exposing executables for environment {}",
            env_name.fancy_display()
        );

        state_changes |= create_executable_trampolines(&script_mapping, &prefix, env_name).await?;

        Ok(state_changes)
    }

    /// Syncs the parsed environment with the installation.
    /// Returns the state_changes if it succeeded, or an error if it didn't.
    pub async fn sync_environment(
        &self,
        env_name: &EnvironmentName,
        removed_packages: Option<Vec<PackageName>>,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::new_with_env(env_name.clone());
        if self.environment_in_sync(env_name).await? {
            tracing::debug!(
                "Environment {} specs already up to date with global manifest",
                env_name.fancy_display()
            );
        } else {
            tracing::debug!(
                "Environment {} specs not up to date with global manifest",
                env_name.fancy_display()
            );
            let mut environment_update = self.install_environment(env_name).await?;

            if let Some(removed_packages) = removed_packages {
                environment_update.add_removed_packages(removed_packages.to_vec());
            };

            state_changes.insert_change(
                env_name,
                StateChange::UpdatedEnvironment(environment_update),
            );
        }

        // Expose executables
        state_changes |= self.expose_executables_from_environment(env_name).await?;

        // Sync shortcuts
        state_changes |= self.sync_shortcuts(env_name).await?;

        // Sync completions
        state_changes |= self.sync_completions(env_name).await?;

        Ok(state_changes)
    }

    /// Delete scripts in the bin folder that are broken
    pub async fn remove_broken_files(&self) -> miette::Result<()> {
        // Get all the files in the global binary directory
        // If there's a trampoline that couldn't be correctly parsed, remove it
        let root_path = self.bin_dir.path();
        let mut entries = tokio_fs::read_dir(&root_path).await.into_diagnostic()?;

        while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
            let path = entry.path();
            if path.is_file() && path.is_executable() && Trampoline::is_trampoline(&path).await? {
                let exposed_name = Trampoline::name(&path)?;
                match Configuration::from_root_path(root_path, &exposed_name).await {
                    Ok(_) => (),
                    Err(ConfigurationParseError::ReadError(config_path, err)) => {
                        tracing::warn!("Couldn't read {}\n{err:?}", config_path.display());
                        tracing::warn!("Removing the trampoline at {}", path.display());
                        tokio_fs::remove_file(path).await.into_diagnostic()?;
                    }
                    Err(ConfigurationParseError::ParseError(config_path, err)) => {
                        tracing::warn!("Couldn't parse {}\n{err:?}", config_path.display());
                        tracing::warn!(
                            "Removing the trampoline at {} and configuration at {}",
                            path.display(),
                            config_path.display()
                        );
                        tokio_fs::remove_file(path).await.into_diagnostic()?;
                        tokio_fs::remove_file(config_path).await.into_diagnostic()?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Delete all non required environments
    pub async fn prune_old_environments(&self) -> miette::Result<StateChanges> {
        let env_set: HashSet<&EnvironmentName> = self.environments().keys().collect();

        let mut state_changes = StateChanges::default();
        for env_path in self.env_root.directories().await? {
            let Some(Ok(env_name)) = env_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(EnvironmentName::from_str)
            else {
                continue;
            };

            if !env_set.contains(&env_name) {
                // Test if the environment directory is a conda environment
                if let Ok(true) = env_path.join(consts::CONDA_META_DIR).try_exists() {
                    // Remove all shortcuts, using the information still available in the
                    // environment
                    state_changes |= self.remove_shortcuts(&env_name).await?;

                    // Remove the conda environment
                    tokio_fs::remove_dir_all(&env_path)
                        .await
                        .into_diagnostic()?;
                    // Get all removable binaries related to the environment
                    let (to_remove, _to_add) = expose_scripts_sync_status(
                        &self.bin_dir,
                        &EnvDir::from_path(env_path.clone()),
                        &IndexSet::new(),
                    )
                    .await?;

                    // Remove all removable binaries
                    for binary_path in to_remove {
                        binary_path.remove().await?;
                    }
                    state_changes.insert_change(&env_name, StateChange::RemovedEnvironment);
                }
            }
        }
        Ok(state_changes)
    }

    /// Install shortcuts of a specific environment
    pub async fn sync_shortcuts(&self, env_name: &EnvironmentName) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();
        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;

        let prefix = self.environment_prefix(env_name).await?;
        let prefix_records = prefix.find_installed_packages()?;

        let shortcuts = environment.shortcuts.clone().unwrap_or_default();
        let (records_to_install, records_to_uninstall) =
            shortcuts_sync_status(shortcuts, prefix_records, prefix.root())?;

        for record in records_to_install {
            rattler_menuinst::install_menuitems_for_record(
                prefix.root(),
                &record,
                environment.platform.unwrap_or(Platform::current()),
                MenuMode::User,
            )
            .into_diagnostic()?;

            state_changes.insert_change(
                env_name,
                StateChange::InstalledShortcut(
                    record
                        .repodata_record
                        .package_record
                        .name
                        .as_normalized()
                        .to_owned(),
                ),
            );
        }

        for record in records_to_uninstall {
            rattler_menuinst::remove_menuitems_for_record(prefix.root(), record.clone())
                .into_diagnostic()?;

            state_changes.insert_change(
                env_name,
                StateChange::UninstalledShortcut(
                    record
                        .repodata_record
                        .package_record
                        .name
                        .as_normalized()
                        .to_owned(),
                ),
            );
        }

        Ok(state_changes)
    }

    /// Remove the shortcuts from the system coming from a specific environment
    pub async fn remove_shortcuts(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();

        // Find menu items in the prefix
        let prefix = self.environment_prefix(env_name).await?;
        let prefix_records = prefix.find_installed_packages()?;

        // Remove menu items
        for record in prefix_records {
            rattler_menuinst::remove_menu_items(&record.installed_system_menus)
                .into_diagnostic()?;
            tracing::info!("Uninstalled menu items for: '{}'", record.file_name());
            state_changes.insert_change(
                env_name,
                StateChange::UninstalledShortcut(record.file_name().to_string()),
            );
        }
        Ok(state_changes)
    }

    #[cfg(unix)] // Completions are only supported on unix like systems
    pub async fn sync_completions(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();

        let environment = self.environment(env_name).ok_or(miette::miette!(
            "Environment {} not found in manifest.",
            env_name.fancy_display()
        ))?;
        let prefix = self.environment_prefix(env_name).await?;
        let execs_all = self
            .executables_of_all_dependencies(env_name)
            .await?
            .into_iter()
            .map(|exec| exec.name)
            .collect();

        let completions_dir = crate::global::completions::CompletionsDir::from_env().await?;
        let (completions_to_remove, completions_to_add) =
            super::completions::completions_sync_status(
                environment.exposed.clone(),
                execs_all,
                prefix.root(),
                &completions_dir,
            )
            .await?;

        for completion_to_remove in completions_to_remove {
            let state_change = completion_to_remove.remove().await?;
            state_changes.insert_change(env_name, state_change);
        }

        for completion_to_add in completions_to_add {
            let Some(state_change) = completion_to_add.install().await? else {
                continue;
            };

            state_changes.insert_change(env_name, state_change);
        }

        Ok(state_changes)
    }

    #[cfg(not(unix))]
    pub async fn sync_completions(
        &self,
        _env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let state_changes = StateChanges::default();
        Ok(state_changes)
    }

    /// Returns a semaphore than can be used to limit the number of concurrent
    /// according to the user configuration.
    fn concurrent_downloads_semaphore(&self) -> Arc<Semaphore> {
        self.concurrent_downloads_semaphore
            .get_or_init(|| {
                let max_concurrent_downloads = self.config().max_concurrent_downloads();
                Arc::new(Semaphore::new(max_concurrent_downloads))
            })
            .clone()
    }

    /// Returns the command dispatcher for this project.
    fn command_dispatcher(&self) -> miette::Result<&CommandDispatcher> {
        const BUILD_DIR: &str = "bld";

        self.command_dispatcher.get_or_try_init(|| {
            let multi_progress = global_multi_progress();
            let anchor_pb = multi_progress.add(ProgressBar::hidden());
            let cache_dirs = pixi_command_dispatcher::CacheDirs::new(
                pixi_config::get_cache_dir()
                    .map(|cache_dir| cache_dir.join(BUILD_DIR))
                    .map_err(|e| miette::miette!("Failed to get cache directory: {}", e))?,
            );

            Ok(pixi_command_dispatcher::CommandDispatcher::builder()
                .with_gateway(self.repodata_gateway()?.clone())
                .with_cache_dirs(cache_dirs)
                .with_root_dir(self.root.clone())
                .with_download_client(self.authenticated_client()?.clone())
                .with_max_download_concurrency(self.concurrent_downloads_semaphore())
                .with_limits(Limits {
                    max_concurrent_solves: self.config().max_concurrent_solves().into(),
                    ..Limits::default()
                })
                .with_backend_overrides(BackendOverride::from_env()?.unwrap_or_default())
                .execute_link_scripts(match self.config.run_post_link_scripts() {
                    RunPostLinkScripts::Insecure => true,
                    RunPostLinkScripts::False => false,
                })
                .with_reporter(TopLevelProgress::new(multi_progress, anchor_pb))
                .finish())
        })
    }
}

impl Repodata for Project {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> miette::Result<&Gateway> {
        self.repodata_gateway.get_or_try_init(|| {
            let client = self.authenticated_client()?.clone();
            let concurrent_downloads = self.concurrent_downloads_semaphore();
            Ok(self
                .config()
                .gateway()
                .with_client(client)
                .with_max_concurrent_requests(concurrent_downloads)
                .finish())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, io::Write};

    use fake::{Fake, faker::filesystem::en::FilePath};
    use itertools::Itertools;
    use rattler_conda_types::{
        NamedChannelOrUrl, PackageRecord, Platform, RepoDataRecord, VersionWithSource,
    };
    use tempfile::tempdir;
    use url::Url;

    use super::*;
    use crate::global::trampoline::{Configuration, Trampoline};

    const SIMPLE_MANIFEST: &str = r#"
        [envs.python]
        channels = ["dummy-channel"]
        [envs.python.dependencies]
        dummy = "3.11.*"
        [envs.python.exposed]
        dummy = "dummy"
        "#;

    #[tokio::test]
    async fn test_project_from_str() {
        let manifest_path: PathBuf = FilePath().fake();
        let env_root = EnvRoot::from_env().await.unwrap();
        let bin_dir = BinDir::from_env().await.unwrap();

        let project =
            Project::from_str(&manifest_path, SIMPLE_MANIFEST, env_root, bin_dir).unwrap();
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[tokio::test]
    async fn test_project_from_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(consts::GLOBAL_MANIFEST_DEFAULT_NAME);

        let env_root = EnvRoot::from_env().await.unwrap();
        let bin_dir = BinDir::from_env().await.unwrap();

        // Create and write global manifest
        let mut file = fs::File::create(&manifest_path).unwrap();
        file.write_all(SIMPLE_MANIFEST.as_bytes()).unwrap();
        let project = Project::from_path(&manifest_path, env_root, bin_dir).unwrap();

        // Canonicalize both paths
        let canonical_root = project.root.canonicalize().unwrap();
        let canonical_manifest_parent = manifest_path.parent().unwrap().canonicalize().unwrap();

        assert_eq!(canonical_root, canonical_manifest_parent);
    }

    #[tokio::test]
    async fn test_project_from_manifest() {
        let manifest_path: PathBuf = FilePath().fake();

        let env_root = EnvRoot::from_env().await.unwrap();
        let bin_dir = BinDir::from_env().await.unwrap();

        let manifest = Manifest::from_str(&manifest_path, SIMPLE_MANIFEST).unwrap();
        let project = Project::from_manifest(manifest, env_root, bin_dir);
        assert_eq!(project.root, manifest_path.parent().unwrap());
    }

    #[test]
    fn test_project_manifest_dir() {
        Project::manifest_dir().unwrap();
    }

    #[tokio::test]
    async fn test_prune_exposed() {
        let tempdir = tempfile::tempdir().unwrap();
        let project = Project::from_str(
            &PathBuf::from("dummy"),
            r#"
            [envs.test]
            channels = ["conda-forge"]
            [envs.test.dependencies]
            python = "*"
            [envs.test.exposed]
            python = "python"
            "#,
            EnvRoot::new(tempdir.path().to_path_buf()).unwrap(),
            BinDir::new(tempdir.path().to_path_buf()).unwrap(),
        )
        .unwrap();

        let env_name = "test".parse().unwrap();

        // Create non-exposed but related binary
        let non_exposed_name = ExposedName::from_str("not-python").unwrap();

        let non_exposed_env_path = if cfg!(windows) {
            project.env_root.path().join("test/bin/not-python.exe")
        } else {
            project.env_root.path().join("test/bin/not-python")
        };

        tokio_fs::create_dir_all(non_exposed_env_path.parent().unwrap())
            .await
            .unwrap();
        tokio_fs::File::create(&non_exposed_env_path).await.unwrap();

        let non_exposed_manifest =
            Configuration::new(non_exposed_env_path, String::new(), HashMap::new());
        let non_exposed_trampoline = Trampoline::new(
            non_exposed_name.clone(),
            project.bin_dir.path().to_path_buf(),
            non_exposed_manifest,
        );

        // write it's trampoline and manifest
        non_exposed_trampoline.save().await.unwrap();

        // Create exposed binary
        let python = ExposedName::from_str("python").unwrap();
        let python_exposed_env_path = if cfg!(windows) {
            project.env_root.path().join("test/bin/python.exe")
        } else {
            project.env_root.path().join("test/bin/python")
        };

        tokio_fs::create_dir_all(python_exposed_env_path.parent().unwrap())
            .await
            .unwrap();
        tokio_fs::File::create(&python_exposed_env_path)
            .await
            .unwrap();

        let exposed_manifest =
            Configuration::new(python_exposed_env_path, String::new(), HashMap::new());
        let exposed_trampoline = Trampoline::new(
            python,
            project.bin_dir.path().to_path_buf(),
            exposed_manifest,
        );

        exposed_trampoline.save().await.unwrap();

        // Create unrelated file
        let unrelated = project.bin_dir.path().join("unrelated");
        fs::File::create(&unrelated).unwrap();

        // Remove exposed
        let state_changes = project.prune_exposed(&env_name).await.unwrap();
        assert_eq!(
            state_changes.changes(),
            std::collections::HashMap::from([(
                env_name.clone(),
                vec![StateChange::RemovedExposed(non_exposed_name)]
            )])
        );

        // Check if the non-exposed file was removed
        // it should be : exposed binary + it's manifest and non related file
        assert_eq!(fs::read_dir(project.bin_dir.path()).unwrap().count(), 3);
        assert!(exposed_trampoline.path().exists());
        assert!(unrelated.exists());
        assert!(!non_exposed_trampoline.path().exists());
    }

    #[tokio::test]
    async fn test_prune() {
        // Create a temporary directory
        let temp_dir = tempdir().unwrap();

        // Set the env root to the temporary directory
        let env_root = EnvRoot::new(temp_dir.path().to_owned()).unwrap();

        // Create some directories in the temporary directory
        let envs = ["env1", "env2", "env3", "non-conda-env-dir"];
        for env in envs {
            EnvDir::from_env_root(env_root.clone(), &EnvironmentName::from_str(env).unwrap())
                .await
                .unwrap();
        }
        // Add conda meta data to env2 to make sure it's seen as a conda environment
        tokio_fs::create_dir_all(env_root.path().join("env2").join(consts::CONDA_META_DIR))
            .await
            .unwrap();

        // Create project with env1 and env3
        let manifest = Manifest::from_str(
            &env_root.path().join(consts::GLOBAL_MANIFEST_DEFAULT_NAME),
            r#"
            [envs.env1]
            channels = ["conda-forge"]
            [envs.env1.dependencies]
            python = "*"
            [envs.env1.exposed]
            python1 = "python"

            [envs.env3]
            channels = ["conda-forge"]
            [envs.env3.dependencies]
            python = "*"
            [envs.env3.exposed]
            python2 = "python"
            "#,
        )
        .unwrap();
        let project = Project::from_manifest(
            manifest,
            env_root.clone(),
            BinDir::new(env_root.path().parent().unwrap().to_path_buf()).unwrap(),
        );

        // Call the prune method with a list of environments to keep (env1 and env3) but
        // not env4
        let state_changes = project.prune_old_environments().await.unwrap();
        assert_eq!(
            state_changes.changes(),
            HashMap::from([(
                "env2".parse().unwrap(),
                vec![StateChange::RemovedEnvironment]
            )])
        );

        // Verify that only the specified directories remain
        let remaining_dirs = fs::read_dir(env_root.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().into_string().unwrap())
            .sorted()
            .collect_vec();

        assert_eq!(remaining_dirs, vec!["env1", "env3", "non-conda-env-dir"]);
    }

    #[test]
    fn test_convert_repodata_to_exposed_data() {
        let temp_dir = tempdir().unwrap();
        let channel_config = ChannelConfig::default_with_root_dir(temp_dir.path().to_owned());
        let mut package_record = PackageRecord::new(
            "python".parse().unwrap(),
            VersionWithSource::from_str("3.9.7").unwrap(),
            "build_string".to_string(),
        );

        // Set platform to something different than current
        package_record.subdir = Platform::LinuxRiscv32.to_string();

        let repodata_record = RepoDataRecord {
            package_record: package_record.clone(),
            file_name: "doesnt_matter.conda".to_string(),
            url: Url::from_str("https://also_doesnt_matter").unwrap(),
            channel: Some(format!(
                "{}{}",
                channel_config.channel_alias.clone(),
                "test-channel"
            )),
        };
        let prefix_record = PrefixRecord::from_repodata_record(
            repodata_record,
            None,
            None,
            vec![],
            Default::default(),
            None,
        );

        // Test with default channel alias
        let (platform, channel, package) =
            convert_record_to_metadata(&prefix_record, &channel_config).unwrap();
        assert_eq!(
            channel,
            NamedChannelOrUrl::from_str("test-channel").unwrap().into()
        );
        assert_eq!(package, "python".parse().unwrap());
        assert_eq!(platform, Some(Platform::LinuxRiscv32));

        // Test with different from default channel alias
        let repodata_record = RepoDataRecord {
            package_record: package_record.clone(),
            file_name: "doesnt_matter.conda".to_string(),
            url: Url::from_str("https://also_doesnt_matter").unwrap(),
            channel: Some("https://test-channel.com/idk".to_string()),
        };
        let prefix_record = PrefixRecord::from_repodata_record(
            repodata_record,
            None,
            None,
            vec![],
            Default::default(),
            None,
        );

        let (_platform, channel, package) =
            convert_record_to_metadata(&prefix_record, &channel_config).unwrap();
        assert_eq!(
            channel,
            NamedChannelOrUrl::from_str("https://test-channel.com/idk")
                .unwrap()
                .into()
        );
        assert_eq!(package, "python".parse().unwrap());
    }
}
