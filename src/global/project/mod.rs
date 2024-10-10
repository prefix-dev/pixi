use super::{extract_executable_from_script, BinDir, EnvRoot, StateChange, StateChanges};
use crate::global::common::{channel_url_to_prioritized_channel, find_package_records};
use crate::global::install::{
    create_activation_script, create_executable_scripts, script_exec_mapping,
};
use crate::global::project::environment::{
    environment_specs_in_sync, get_expose_scripts_sync_status,
};
use crate::repodata::Repodata;
use crate::rlimit::try_increase_rlimit_to_sensible;
use crate::{
    global::{common::is_text, find_executables, EnvDir},
    prefix::Prefix,
};
use ahash::HashSet;
pub(crate) use environment::EnvironmentName;
use fancy_display::FancyDisplay;
use fs::tokio as tokio_fs;
use fs_err as fs;
use futures::stream::StreamExt;
use indexmap::{IndexMap, IndexSet};
pub(crate) use manifest::{Manifest, Mapping};
use miette::{miette, Context, IntoDiagnostic};
pub(crate) use parsed_manifest::ExposedName;
pub(crate) use parsed_manifest::ParsedEnvironment;
use parsed_manifest::ParsedManifest;
use pixi_config::{default_channel_config, home_path, Config};
use pixi_consts::consts;
use pixi_manifest::PrioritizedChannel;
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use pixi_utils::executable_from_path;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler::install::{DefaultProgressFormatter, IndicatifReporter, Installer};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{
    ChannelConfig, GenericVirtualPackage, MatchSpec, PackageName, Platform, PrefixRecord,
};
use rattler_lock::Matches;
use rattler_repodata_gateway::Gateway;
use rattler_shell::shell::ShellEnum;
use rattler_solve::resolvo::Solver;
use rattler_solve::{SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest_middleware::ClientWithMiddleware;
use std::sync::OnceLock;
use std::{
    ffi::OsStr,
    fmt::{Debug, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};
use toml_edit::DocumentMut;

mod environment;
mod manifest;
mod parsed_manifest;

pub(crate) const MANIFEST_DEFAULT_NAME: &str = "pixi-global.toml";
pub(crate) const MANIFESTS_DIR: &str = "manifests";

/// The pixi global project, this main struct to interact with the pixi global
/// project. This struct holds the `Manifest` and has functions to modify
/// or request information from it. This allows in the future to have multiple
/// manifests linked to a pixi global project.
#[derive(Clone)]
pub struct Project {
    /// Root folder of the project
    root: PathBuf,
    /// The manifest for the project
    pub(crate) manifest: Manifest,
    /// The global configuration as loaded from the config file(s)
    config: Config,
    /// Root directory of the global environments
    pub(crate) env_root: EnvRoot,
    /// Binary directory
    pub(crate) bin_dir: BinDir,
    /// Reqwest client shared for this project.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    client: OnceLock<(reqwest::Client, ClientWithMiddleware)>,
    /// The repodata gateway to use for answering queries about repodata.
    /// This is wrapped in a `OnceLock` to allow for lazy initialization.
    repodata_gateway: OnceLock<Gateway>,
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
    /// Constructs an `ExposedData` instance from a exposed script path.
    ///
    /// This function extracts metadata from the exposed script path, including the
    /// environment name, platform, channel, and package information, by reading
    /// the associated `conda-meta` directory.
    pub async fn from_exposed_path(
        path: &Path,
        env_root: &EnvRoot,
        channel_config: &ChannelConfig,
    ) -> miette::Result<Self> {
        let exposed = ExposedName::from_str(executable_from_path(path).as_str())?;
        let executable_path = extract_executable_from_script(path).await?;

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
            .find_installed_packages(None)
            .await?
            .iter()
            .map(|prefix_record| prefix_record.repodata_record.channel.clone())
            .collect::<HashSet<_>>();
        for channel in all_channels {
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

/// Converts a `PrefixRecord` into package metadata, including platform, channel, and package name.
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

    let channel =
        channel_url_to_prioritized_channel(&prefix_record.repodata_record.channel, channel_config)?;

    Ok((platform, channel, package_name))
}

/// Extracts package metadata from the `conda-meta` directory for a given executable.
///
/// This function reads the `conda-meta` directory to find the package metadata
/// associated with the specified executable. It returns the platform, channel, and
/// package name of the executable.
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

        let client = OnceLock::new();
        let repodata_gateway = OnceLock::new();
        Self {
            root,
            manifest,
            config,
            env_root,
            bin_dir,
            client,
            repodata_gateway,
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
    pub(crate) async fn discover_or_create() -> miette::Result<Self> {
        let manifest_dir = Self::manifest_dir()?;
        let manifest_path = manifest_dir.join(MANIFEST_DEFAULT_NAME);
        // Prompt user if the manifest is empty and the user wants to create one

        let bin_dir = BinDir::from_env().await?;
        let env_root = EnvRoot::from_env().await?;

        if !manifest_path.exists() {
            tokio_fs::create_dir_all(&manifest_dir)
                .await
                .into_diagnostic()?;

            if !env_root.directories().await?.is_empty() {
                return Self::try_from_existing_installation(&manifest_path, env_root, bin_dir)
                    .await
                    .wrap_err_with(|| {
                        "Failed to create global manifest from existing installation"
                    });
            } else {
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
            .files()
            .await?
            .into_iter()
            .filter_map(|path| match is_text(&path) {
                Ok(true) => Some(Ok(path)), // Success and is text, continue with path
                Ok(false) => None,          // Success and isn't text, filter out
                Err(e) => Some(Err(e)),     // Failure, continue with error
            })
            .map(|result| async {
                match result {
                    Ok(path) => {
                        ExposedData::from_exposed_path(
                            &path,
                            &env_root,
                            config.global_channel_config(),
                        )
                        .await
                    }
                    Err(e) => Err(e),
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
    pub(crate) fn manifest_dir() -> miette::Result<PathBuf> {
        home_path()
            .map(|dir| dir.join(MANIFESTS_DIR))
            .ok_or_else(|| miette::miette!("Couldn't get home directory"))
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
    pub(crate) fn with_cli_config<C>(mut self, config: C) -> Self
    where
        C: Into<Config>,
    {
        self.config = self.config.merge_config(config.into());
        self
    }

    /// Returns the environments in this project.
    pub(crate) fn environments(&self) -> &IndexMap<EnvironmentName, ParsedEnvironment> {
        &self.manifest.parsed.envs
    }

    /// Return the environment with the given name.
    pub(crate) fn environment(&self, name: &EnvironmentName) -> Option<&ParsedEnvironment> {
        self.manifest.parsed.envs.get(name)
    }

    /// Returns the prefix of the environment with the given name.
    pub(crate) async fn environment_prefix(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<Prefix> {
        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;
        Ok(Prefix::new(env_dir.path()))
    }

    /// Create an authenticated reqwest client for this project
    /// use authentication from `rattler_networking`
    pub fn authenticated_client(&self) -> &ClientWithMiddleware {
        &self.client_and_authenticated_client().1
    }

    fn client_and_authenticated_client(&self) -> &(reqwest::Client, ClientWithMiddleware) {
        self.client
            .get_or_init(|| build_reqwest_clients(Some(&self.config)))
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) async fn install_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<()> {
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

        let match_specs = environment
            .dependencies
            .clone()
            .into_iter()
            .map(|(name, spec)| {
                if let Some(nameless_spec) = spec
                    .clone()
                    .try_into_nameless_match_spec(self.config().global_channel_config())
                    .into_diagnostic()?
                {
                    Ok(MatchSpec::from_nameless(nameless_spec, Some(name.clone())))
                } else {
                    Err(miette!("Couldn't convert {spec:?} to nameless match spec."))
                }
            })
            .collect::<miette::Result<Vec<MatchSpec>>>()?;

        let repodata = await_in_progress(
            format!("Querying repodata for {} ", env_name.fancy_display()),
            |_| async {
                self.repodata_gateway()
                    .query(channels, [platform, Platform::NoArch], match_specs.clone())
                    .recursive(true)
                    .await
                    .into_diagnostic()
            },
        )
        .await?;

        // Determine virtual packages of the current platform
        let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::default())
            .into_diagnostic()
            .context("failed to determine virtual packages")?
            .iter()
            .cloned()
            .map(GenericVirtualPackage::from)
            .collect();

        // Solve the environment
        let solved_records = tokio::task::spawn_blocking(move || {
            wrap_in_progress("solving environment", move || {
                Solver.solve(SolverTask {
                    specs: match_specs,
                    virtual_packages,
                    ..SolverTask::from_iter(&repodata)
                })
            })
            .into_diagnostic()
            .context("failed to solve environment")
        })
        .await
        .into_diagnostic()??;

        try_increase_rlimit_to_sensible();

        // Install the environment
        let package_cache = PackageCache::new(pixi_config::get_cache_dir()?.join("pkgs"));
        let prefix = self.environment_prefix(env_name).await?;
        await_in_progress(
            format!(
                "Creating virtual environment for {}",
                env_name.fancy_display()
            ),
            |pb| {
                Installer::new()
                    .with_download_client(self.authenticated_client().clone())
                    .with_io_concurrency_limit(100)
                    .with_execute_link_scripts(false)
                    .with_package_cache(package_cache)
                    .with_target_platform(platform)
                    .with_reporter(
                        IndicatifReporter::builder()
                            .with_multi_progress(global_multi_progress())
                            .with_placement(rattler::install::Placement::After(pb))
                            .with_formatter(DefaultProgressFormatter::default().with_prefix("  "))
                            .clear_when_done(true)
                            .finish(),
                    )
                    .install(prefix.root(), solved_records)
            },
        )
        .await
        .into_diagnostic()?;

        Ok(())
    }

    /// Find all binaries related to the environment and remove those that are not listed as exposed.
    pub async fn prune_exposed(&self, env_name: &EnvironmentName) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();
        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;
        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;

        // Get all removable binaries related to the environment
        let (to_remove, _to_add) =
            get_expose_scripts_sync_status(&self.bin_dir, &env_dir, &environment.exposed).await?;

        // Remove all removable binaries
        for exposed_path in to_remove {
            state_changes.push_change(StateChange::RemovedExposed(
                ExposedName::from_str(&executable_from_path(&exposed_path))?,
                env_name.clone(),
            ));
            tokio_fs::remove_file(&exposed_path)
                .await
                .into_diagnostic()?;
        }

        Ok(state_changes)
    }

    /// Check if the environment is in sync with the manifest
    ///
    /// Validated the specs in the installed environment.
    /// And verifies only and all required exposed binaries are in the bin dir.
    pub async fn environment_in_sync(&self, env_name: &EnvironmentName) -> miette::Result<bool> {
        let environment = self.environment(env_name).ok_or(miette::miette!(
            "Environment {} not found in manifest.",
            env_name.fancy_display()
        ))?;

        let specs = environment
            .dependencies
            .clone()
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

        let env_dir =
            EnvDir::from_path(self.env_root.clone().path().join(env_name.clone().as_str()));

        let specs_in_sync =
            environment_specs_in_sync(&env_dir, &specs, environment.platform).await?;
        if !specs_in_sync {
            return Ok(false);
        }

        // Verify the binaries to be in sync with the environment
        let (to_remove, to_add) =
            get_expose_scripts_sync_status(&self.bin_dir, &env_dir, &environment.exposed).await?;
        if !to_remove.is_empty() || !to_add.is_empty() {
            tracing::debug!(
                "Environment {} binaries not in sync: to_remove: {:?}, to_add: {:?}",
                env_name.fancy_display(),
                to_remove,
                to_add
            );
            return Ok(false);
        }

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
    /// This function will first remove all binaries that are not listed as exposed.
    /// It will then create an activation script for the shell and create the scripts.
    pub async fn expose_executables_from_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();

        // First clean up binaries that are not listed as exposed
        state_changes |= self.prune_exposed(env_name).await?;

        // Determine the shell to use for the invocation script
        let shell: ShellEnum = if cfg!(windows) {
            rattler_shell::shell::CmdExe.into()
        } else {
            rattler_shell::shell::Bash.into()
        };
        let env_dir = EnvDir::from_env_root(self.env_root.clone(), env_name).await?;
        let prefix = Prefix::new(env_dir.path());

        let environment = self
            .environment(env_name)
            .ok_or_else(|| miette::miette!("Environment {} not found", env_name.fancy_display()))?;

        // Construct the reusable activation script for the shell and generate an
        // invocation script for each executable added by the package to the
        // environment.
        let activation_script = create_activation_script(&prefix, shell.clone())?;

        let prefix_records = &prefix.find_installed_packages(None).await?;

        let all_executables = &prefix.find_executables(prefix_records.as_slice());

        let exposed: HashSet<&str> = environment
            .exposed
            .iter()
            .map(|map| map.executable_name())
            .collect();

        let exposed_executables: Vec<_> = all_executables
            .iter()
            .filter(|(name, _)| exposed.contains(name.as_str()))
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
        state_changes |= create_executable_scripts(
            &script_mapping,
            &prefix,
            &shell,
            activation_script,
            env_name,
        )
        .await?;

        Ok(state_changes)
    }

    // Syncs the manifest with the local environments
    // Returns true if the global installation had to be updated
    pub(crate) async fn sync(&self) -> Result<StateChanges, miette::Error> {
        let mut state_changes = StateChanges::default();

        // Prune environments that are not listed
        state_changes |= self.prune_old_environments().await?;

        for (env_name, _parsed_environment) in self.environments() {
            state_changes |= self.sync_environment(env_name).await?;
        }

        Ok(state_changes)
    }

    /// Syncs the parsed environment with the installation.
    /// Returns true if the environment had to be updated.
    pub(crate) async fn sync_environment(
        &self,
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();
        if !self.environment_in_sync(env_name).await? {
            tracing::debug!(
                "Environment {} specs not up to date with manifest",
                env_name.fancy_display()
            );
            self.install_environment(env_name).await?;
            state_changes.set_has_updated(true);
        }

        // Expose executables
        state_changes |= self.expose_executables_from_environment(env_name).await?;

        Ok(state_changes)
    }

    /// Delete all non required environments
    pub(crate) async fn prune_old_environments(&self) -> miette::Result<StateChanges> {
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
                    // Remove the conda environment
                    tokio_fs::remove_dir_all(&env_path)
                        .await
                        .into_diagnostic()?;
                    // Get all removable binaries related to the environment
                    let (to_remove, _to_add) = get_expose_scripts_sync_status(
                        &self.bin_dir,
                        &EnvDir::from_path(env_path.clone()),
                        &IndexSet::new(),
                    )
                    .await?;

                    // Remove all removable binaries
                    for binary_path in to_remove {
                        tokio_fs::remove_file(&binary_path)
                            .await
                            .into_diagnostic()?;
                    }
                    state_changes.push_change(StateChange::RemovedEnvironment(env_name));
                }
            }
        }
        Ok(state_changes)
    }

    // Figure which packages have been added
    pub async fn added_packages(
        &self,
        specs: &[MatchSpec],
        env_name: &EnvironmentName,
    ) -> miette::Result<StateChanges> {
        let mut state_changes = StateChanges::default();
        state_changes.push_changes(
            self.environment_prefix(env_name)
                .await?
                .find_installed_packages(None)
                .await?
                .into_iter()
                .filter(|r| specs.iter().any(|s| s.matches(&r.repodata_record)))
                .map(|r| r.repodata_record.package_record)
                .map(|record| StateChange::AddedPackage(record, env_name.clone())),
        );
        Ok(state_changes)
    }
}

impl Repodata for Project {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> &Gateway {
        self.repodata_gateway.get_or_init(|| {
            Self::repodata_gateway_init(
                self.authenticated_client().clone(),
                self.config().clone().into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use fake::{faker::filesystem::zh_tw::FilePath, Fake};
    use itertools::Itertools;
    use rattler_conda_types::{
        NamedChannelOrUrl, PackageRecord, Platform, RepoDataRecord, VersionWithSource,
    };
    use tempfile::tempdir;
    use url::Url;

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
        let manifest_path = tempdir.path().join(MANIFEST_DEFAULT_NAME);

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
        let not_python = ExposedName::from_str("not-python").unwrap();
        let non_exposed_bin = project.bin_dir.executable_script_path(&not_python);
        let mut file = fs::File::create(&non_exposed_bin).unwrap();
        #[cfg(unix)]
        {
            let path = project.env_root.path().join("test/bin/not-python");
            file.write_all(format!(r#""{}" "$@""#, path.to_string_lossy()).as_bytes())
                .unwrap();
        }
        #[cfg(windows)]
        {
            let path = project.env_root.path().join("test/bin/not-python.exe");
            file.write_all(format!(r#"@"{}" %*"#, path.to_string_lossy()).as_bytes())
                .unwrap();
        }

        // Create a file that should be exposed
        let python = ExposedName::from_str("python").unwrap();
        let bin = project.bin_dir.executable_script_path(&python);
        let mut file = fs::File::create(&bin).unwrap();
        #[cfg(unix)]
        {
            let path = project.env_root.path().join("test/bin/python");
            file.write_all(format!(r#""{}" "$@""#, path.to_string_lossy()).as_bytes())
                .unwrap();
        }
        #[cfg(windows)]
        {
            let path = project.env_root.path().join("test/bin/python.exe");
            file.write_all(format!(r#"@"{}" %*"#, path.to_string_lossy()).as_bytes())
                .unwrap();
        }

        // Create unrelated file
        let unrelated = project.bin_dir.path().join("unrelated");
        fs::File::create(&unrelated).unwrap();

        // Remove exposed
        let state_changes = project.prune_exposed(&env_name).await.unwrap();
        assert_eq!(
            state_changes.changes(),
            vec![StateChange::RemovedExposed(not_python, env_name.clone())]
        );

        // Check if the non-exposed file was removed
        assert_eq!(fs::read_dir(project.bin_dir.path()).unwrap().count(), 2);
        assert!(bin.exists());
        assert!(unrelated.exists());
        assert!(!non_exposed_bin.exists());
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
            &env_root.path().join(MANIFEST_DEFAULT_NAME),
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

        // Call the prune method with a list of environments to keep (env1 and env3) but not env4
        let state_changes = project.prune_old_environments().await.unwrap();
        assert_eq!(
            state_changes.changes(),
            vec![StateChange::RemovedEnvironment("env2".parse().unwrap())]
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
            channel: format!("{}{}", channel_config.channel_alias.clone(), "test-channel"),
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
            channel: "https://test-channel.com/idk".to_string(),
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
