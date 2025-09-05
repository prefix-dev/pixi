use std::{
    cmp::PartialEq,
    collections::{HashMap, HashSet, hash_map::Entry},
    future::{Future, ready},
    iter,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use barrier_cell::BarrierCell;
use dashmap::DashMap;
use fancy_display::FancyDisplay;
use futures::{FutureExt, StreamExt, TryFutureExt, stream::FuturesUnordered};
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::{Diagnostic, IntoDiagnostic, MietteDiagnostic, Report, WrapErr};
use pixi_command_dispatcher::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, PixiEnvironmentSpec,
    SolvePixiEnvironmentError,
};
use pixi_consts::consts;
use pixi_glob::GlobHashCache;
use pixi_manifest::{ChannelPriority, EnvironmentName, FeaturesExt};
use pixi_progress::global_multi_progress;
use pixi_record::{ParseLockFileError, PixiRecord};
use pixi_utils::prefix::Prefix;
use pixi_uv_conversions::{
    ConversionError, to_extra_name, to_marker_environment, to_normalize, to_uv_extra_name,
    to_uv_normalize,
};
use pypi_mapping::{self, MappingClient};
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use rattler::package_cache::PackageCache;
use rattler_conda_types::{Arch, GenericVirtualPackage, PackageName, ParseChannelError, Platform};
use rattler_lock::{LockFile, LockedPackageRef, ParseCondaLockError};
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::Instrument;
use uv_normalize::ExtraName;

use super::{
    CondaPrefixUpdater, InstallSubset, PixiRecordsByName, PypiRecordsByName, UvResolutionContext,
    outdated::OutdatedEnvironments, utils::IoConcurrencyLimit,
};
use crate::{
    Workspace,
    activation::CurrentEnvVarBehavior,
    environment::{
        CondaPrefixUpdated, EnvironmentFile, InstallFilter, LockFileUsage, LockedEnvironmentHash,
        PerEnvironmentAndPlatform, PerGroup, PerGroupAndPlatform, PythonStatus,
        read_environment_file, write_environment_file,
    },
    install_pypi::{PyPIBuildConfig, PyPIContextConfig, PyPIEnvironmentUpdater, PyPIUpdateConfig},
    lock_file::{
        self, PypiRecord, reporter::SolveProgressBar,
        virtual_packages::validate_system_meets_environment_requirements,
    },
    workspace::{
        Environment, EnvironmentVars, HasWorkspaceRef, get_activated_environment_variables,
        grouped_environment::{GroupedEnvironment, GroupedEnvironmentName},
    },
};

impl Workspace {
    /// Ensures that the lock-file is up-to-date with the project.
    ///
    /// This function will return a `LockFileDerivedData` struct that contains
    /// the lock-file and any potential derived data that was computed as
    /// part of this function. The derived data might be usable by other
    /// functions to avoid recomputing the same data.
    ///
    /// This function starts by checking if the lock-file is up-to-date. If it
    /// is not up-to-date it will construct a task graph of all the work
    /// that needs to be done to update the lock-file. The tasks are awaited
    /// in a specific order to make sure that we can start instantiating
    /// prefixes as soon as possible.
    pub async fn update_lock_file(
        &self,
        options: UpdateLockFileOptions,
    ) -> miette::Result<(LockFileDerivedData<'_>, bool)> {
        let lock_file = self.load_lock_file().await?;
        let glob_hash_cache = GlobHashCache::default();

        // Construct a command dispatcher that will be used to run the tasks.
        let multi_progress = global_multi_progress();
        let anchor_pb = multi_progress.add(ProgressBar::hidden());
        let command_dispatcher = self
            .command_dispatcher_builder()?
            .with_reporter(pixi_reporters::TopLevelProgress::new(
                global_multi_progress(),
                anchor_pb,
            ))
            .finish();

        // Get the package cache from the dispatcher.
        let package_cache = command_dispatcher.package_cache().clone();

        // should we check the lock-file in the first place?
        if !options.lock_file_usage.should_check_if_out_of_date() {
            tracing::info!("skipping check if lock-file is up-to-date");

            return Ok((
                LockFileDerivedData {
                    workspace: self,
                    lock_file,
                    package_cache,
                    updated_conda_prefixes: Default::default(),
                    updated_pypi_prefixes: Default::default(),
                    uv_context: Default::default(),
                    io_concurrency_limit: IoConcurrencyLimit::default(),
                    command_dispatcher,
                    glob_hash_cache,
                },
                false,
            ));
        }

        // Check which environments are out of date.
        let outdated = OutdatedEnvironments::from_workspace_and_lock_file(
            self,
            &lock_file,
            glob_hash_cache.clone(),
        )
        .await;
        if outdated.is_empty() {
            tracing::info!("the lock-file is up-to-date");

            // If no-environment is outdated we can return early.
            return Ok((
                LockFileDerivedData {
                    workspace: self,
                    lock_file,
                    package_cache,
                    updated_conda_prefixes: Default::default(),
                    updated_pypi_prefixes: Default::default(),
                    uv_context: Default::default(),
                    io_concurrency_limit: IoConcurrencyLimit::default(),
                    command_dispatcher,
                    glob_hash_cache,
                },
                false,
            ));
        }

        // If the lock-file is out of date, but we're not allowed to update it, we
        // should exit.
        if !options.lock_file_usage.allow_updates() {
            miette::bail!("lock-file not up-to-date with the workspace");
        }

        // Construct an update context and perform the actual update.
        let lock_file_derived_data = UpdateContext::builder(self)
            .with_package_cache(package_cache)
            .with_no_install(options.no_install)
            .with_outdated_environments(outdated)
            .with_lock_file(lock_file)
            .with_glob_hash_cache(glob_hash_cache)
            .with_command_dispatcher(command_dispatcher)
            .finish()
            .await?
            .update()
            .await?;

        // Write the lock-file to disk
        lock_file_derived_data.write_to_disk()?;

        Ok((lock_file_derived_data, true))
    }

    /// Loads the lockfile for the workspace or returns `Lockfile::default` if
    /// none could be found.
    pub async fn load_lock_file(&self) -> miette::Result<LockFile> {
        let lock_file_path = self.lock_file_path();
        if lock_file_path.is_file() {
            // Spawn a background task because loading the file might be IO bound.
            tokio::task::spawn_blocking(move || {
                LockFile::from_path(&lock_file_path)
                    .map_err(|err| match err {
                        ParseCondaLockError::IncompatibleVersion { lock_file_version, max_supported_version } => {
                            miette::miette!(
                            help="Please update pixi to the latest version and try again.",
                            "The lock file version is {}, but only up to including version {} is supported by the current version.",
                            lock_file_version, max_supported_version
                        )
                        }
                        _ => miette::miette!(err),
                    })
                    .wrap_err_with(|| {
                        format!(
                            "Failed to load lock file from `{}`",
                            lock_file_path.display()
                        )
                    })
            })
                .await
                .unwrap_or_else(|e| Err(e).into_diagnostic())
        } else {
            Ok(LockFile::default())
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
enum UpdateError {
    #[error("the lockfile is not up-to-date with requested environment: '{}'", .0.fancy_display())]
    LockFileMissingEnv(EnvironmentName),
    #[error("some information from the lockfile could not be parsed")]
    ParseLockFileError(#[from] ParseLockFileError),
}

#[derive(Debug, Error, Diagnostic)]
pub enum SolveCondaEnvironmentError {
    #[error("failed to solve requirements of environment '{}' for platform '{}'", .environment_name.fancy_display(), .platform)]
    SolveFailed {
        environment_name: GroupedEnvironmentName,
        platform: Platform,
        #[source]
        #[diagnostic_source]
        source: CommandDispatcherError<SolvePixiEnvironmentError>,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    PypiMappingFailed(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    ParseChannels(#[from] ParseChannelError),
}

/// Options to pass to [`Workspace::update_lock_file`].
#[derive(Default)]
pub struct UpdateLockFileOptions {
    /// Defines what to do if the lock-file is out of date
    pub lock_file_usage: LockFileUsage,

    /// Don't install anything to disk.
    pub no_install: bool,

    /// The maximum number of concurrent solves that are allowed to run. If this
    /// value is None a heuristic is used based on the number of cores
    /// available from the system.
    pub max_concurrent_solves: usize,
}

#[derive(Debug, Clone, Default)]
pub enum ReinstallPackages {
    #[default]
    None,
    All,
    Some(HashSet<String>),
}

/// A struct that holds the lock-file and any potential derived data that was
/// computed when calling `update_lock_file`.
pub struct LockFileDerivedData<'p> {
    pub workspace: &'p Workspace,

    /// The lock-file
    ///
    /// Prefer to use `as_lock_file` or `into_lock_file` to also make a decision
    /// what to do with the resources used to create this instance.
    pub lock_file: LockFile,

    /// The package cache
    pub package_cache: PackageCache,

    /// A list of prefixes that are up-to-date with the latest conda packages.
    pub updated_conda_prefixes:
        DashMap<EnvironmentName, Arc<async_once_cell::OnceCell<(Prefix, PythonStatus)>>>,

    /// A list of prefixes that have been updated while resolving all
    /// dependencies.
    pub updated_pypi_prefixes: DashMap<EnvironmentName, Arc<async_once_cell::OnceCell<Prefix>>>,

    /// The cached uv context
    pub uv_context: once_cell::sync::OnceCell<UvResolutionContext>,

    /// The IO concurrency semaphore to use when updating environments
    pub io_concurrency_limit: IoConcurrencyLimit,

    /// The command dispatcher that is used to build and solve.
    pub command_dispatcher: CommandDispatcher,

    /// An object that caches input hashes
    pub glob_hash_cache: GlobHashCache,
}

/// The mode to use when updating a prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    /// Validate if the prefix is up-to-date.
    /// Using a fast and simple validation method.
    /// Used for skipping the update if the prefix is already up-to-date, in
    /// activating commands. Like `pixi shell` or `pixi run`.
    QuickValidate,
    /// Force a prefix install without running the short validation.
    /// Used for updating the prefix when the lock-file likely out of date.
    /// Like `pixi install` or `pixi update`.
    Revalidate,
}

impl<'p> LockFileDerivedData<'p> {
    /// Write the lock-file to disk.
    pub fn write_to_disk(&self) -> miette::Result<()> {
        let lock_file_path = self.workspace.lock_file_path();
        self.lock_file
            .to_path(&lock_file_path)
            .into_diagnostic()
            .context("failed to write lock-file to disk")
    }

    /// Consumes this instance, dropping any resources that are not needed
    /// anymore to work with the lock-file.
    pub fn into_lock_file(self) -> LockFile {
        self.lock_file
    }

    /// Returns a reference to the internal lock-file but does not consume any
    /// build resources, this is useful if you want to keep using the original
    /// instance.
    pub fn as_lock_file(&self) -> &LockFile {
        &self.lock_file
    }

    fn locked_environment_hash(
        &self,
        environment: &Environment<'p>,
    ) -> miette::Result<LockedEnvironmentHash> {
        let locked_environment = self
            .lock_file
            .environment(environment.name().as_str())
            .ok_or_else(|| UpdateError::LockFileMissingEnv(environment.name().clone()))?;
        Ok(LockedEnvironmentHash::from_environment(
            locked_environment,
            environment.best_platform(),
        ))
    }

    /// Returns the up-to-date prefix for the given environment.
    pub async fn prefix(
        &self,
        environment: &Environment<'p>,
        update_mode: UpdateMode,
        reinstall_packages: &ReinstallPackages,
        filter: &InstallFilter,
    ) -> miette::Result<Prefix> {
        // Check if the prefix is already up-to-date by validating the hash with the
        // environment file
        let hash = self.locked_environment_hash(environment)?;
        if update_mode == UpdateMode::QuickValidate {
            if let Some(prefix) = self.cached_prefix(environment, &hash) {
                return prefix;
            }
        }

        // Get the up-to-date prefix
        let prefix = self
            .update_prefix(environment, reinstall_packages, filter)
            .await?;

        // We write an invalid hash when filtering, as we will need to do a full
        // revalidation of the environment anyways
        let hash = if filter.filter_active() {
            LockedEnvironmentHash::invalid()
        } else {
            hash
        };

        // Save an environment file to the environment directory after the update.
        // Avoiding writing the cache away before the update is done.
        write_environment_file(
            &environment.dir(),
            EnvironmentFile {
                manifest_path: environment.workspace().workspace.provenance.path.clone(),
                environment_name: environment.name().to_string(),
                pixi_version: consts::PIXI_VERSION.to_string(),
                environment_lock_file_hash: hash,
            },
        )?;

        Ok(prefix)
    }

    fn cached_prefix(
        &self,
        environment: &Environment<'p>,
        hash: &LockedEnvironmentHash,
    ) -> Option<Result<Prefix, Report>> {
        let Ok(Some(environment_file)) = read_environment_file(&environment.dir()) else {
            tracing::debug!(
                "Environment file not found or parsable for '{}'",
                environment.name().fancy_display()
            );
            return None;
        };

        if environment_file.environment_lock_file_hash == *hash {
            // If we contain source packages from conda or PyPI we update the prefix by
            // default
            let contains_conda_source_pkgs = self.lock_file.environments().any(|(_, env)| {
                env.conda_packages(Platform::current())
                    .is_some_and(|mut packages| {
                        packages.any(|package| package.as_source().is_some())
                    })
            });

            // Check if we have source packages from PyPI
            // that is a directory, this is basically the only kind of source dependency
            // that you'll modify on a general basis.
            let contains_pypi_source_pkgs = environment
                .pypi_dependencies(Some(Platform::current()))
                .iter()
                .any(|(_, req)| {
                    req.iter()
                        .any(|dep| dep.as_path().map(|p| p.is_dir()).unwrap_or_default())
                });
            if contains_conda_source_pkgs || contains_pypi_source_pkgs {
                tracing::debug!(
                    "Lock file contains source packages: ignore lock file hash and update the prefix"
                );
            } else {
                tracing::info!(
                    "Environment '{}' is up-to-date with lock file hash",
                    environment.name().fancy_display()
                );
                return Some(Ok(Prefix::new(environment.dir())));
            }
        }
        None
    }

    /// Returns the up-to-date prefix for the given environment.
    async fn update_prefix(
        &self,
        environment: &Environment<'p>,
        reinstall_packages: &ReinstallPackages,
        filter: &InstallFilter,
    ) -> miette::Result<Prefix> {
        let prefix_once_cell = self
            .updated_pypi_prefixes
            .entry(environment.name().clone())
            .or_default()
            .clone();
        prefix_once_cell
            .get_or_try_init(async {
                let start = Instant::now();

                // Validate the virtual packages for the environment match the system
                validate_system_meets_environment_requirements(
                    &self.lock_file,
                    environment.best_platform(),
                    environment.name(),
                    None,
                )
                .wrap_err(format!(
                    "Cannot install environment '{}'",
                    environment.name().fancy_display()
                ))?;

                let platform = environment.best_platform();
                let locked_env = self.locked_env(environment)?;
                let subset = InstallSubset::new(
                    &filter.skip_with_deps,
                    &filter.skip_direct,
                    &filter.target_packages,
                );
                let result = subset.filter(locked_env.packages(platform))?;
                let packages = result.install;
                let ignored = result.ignore;

                // Separate the packages into conda and pypi packages
                let (conda_packages, pypi_packages) = packages
                    .into_iter()
                    .partition::<Vec<_>, _>(|p| p.as_conda().is_some());

                let (ignored_conda, ignored_pypi): (HashSet<_>, HashSet<_>) =
                    ignored.into_iter().partition_map(|p| match p {
                        LockedPackageRef::Conda(data) => Either::Left(data.record().name.clone()),
                        LockedPackageRef::Pypi(data, _) => Either::Right(data.name.clone()),
                    });

                let pixi_records = locked_packages_to_pixi_records(conda_packages)?;

                let pypi_records = pypi_packages
                    .into_iter()
                    .filter_map(LockedPackageRef::as_pypi)
                    .map(|(data, env_data)| (data.clone(), env_data.clone()))
                    .collect::<Vec<_>>();

                let conda_reinstall_packages = match reinstall_packages {
                    ReinstallPackages::None => None,
                    ReinstallPackages::Some(p) => Some(
                        p.iter()
                            .filter_map(|p| PackageName::from_str(p).ok())
                            .filter(|name| pixi_records.iter().any(|r| r.name() == name))
                            .collect(),
                    ),
                    ReinstallPackages::All => {
                        Some(pixi_records.iter().map(|r| r.name().clone()).collect())
                    }
                };

                // Get the prefix with the conda packages installed.
                let (prefix, python_status) = self
                    .conda_prefix(environment, conda_reinstall_packages, Some(ignored_conda))
                    .await?;

                // No `uv` support for WASM right now
                if platform.arch() == Some(Arch::Wasm32) {
                    return Ok(prefix);
                }

                let pypi_lock_file_names = pypi_records
                    .iter()
                    .filter_map(|(data, _)| to_uv_normalize(&data.name).ok())
                    .collect::<HashSet<_>>();

                // Figure out uv reinstall
                let (uv_reinstall, uv_packages) = match reinstall_packages {
                    ReinstallPackages::None => (Some(false), None),
                    ReinstallPackages::All => (Some(true), None),
                    ReinstallPackages::Some(pkgs) => (
                        None,
                        Some(
                            pkgs.iter()
                                .filter_map(|pkg| uv_pep508::PackageName::from_str(pkg).ok())
                                .filter(|name| pypi_lock_file_names.contains(name))
                                .collect(),
                        ),
                    ),
                };

                let uv_context = self
                    .uv_context
                    .get_or_try_init(|| UvResolutionContext::from_workspace(self.workspace))?
                    .clone()
                    .set_cache_refresh(uv_reinstall, uv_packages);

                // TODO: This can be really slow (~200ms for pixi on @ruben-arts machine).
                let env_variables = get_activated_environment_variables(
                    self.workspace.env_vars(),
                    environment,
                    CurrentEnvVarBehavior::Exclude,
                    None,
                    false,
                    false,
                )
                .await?;

                let non_isolated_packages = environment.pypi_options().no_build_isolation;
                let no_build = environment
                    .pypi_options()
                    .no_build
                    .clone()
                    .unwrap_or_default();
                let no_binary = environment
                    .pypi_options()
                    .no_binary
                    .clone()
                    .unwrap_or_default();

                // Update the prefix with Pypi records
                {
                    let pypi_indexes = self.locked_env(environment)?.pypi_indexes().cloned();
                    let index_strategy = environment.pypi_options().index_strategy.clone();
                    let exclude_newer = environment.exclude_newer();

                    let config = PyPIUpdateConfig {
                        environment_name: environment.name(),
                        prefix: &prefix,
                        platform: environment.best_platform(),
                        lock_file_dir: self.workspace.root(),
                        system_requirements: &environment.system_requirements(),
                    };

                    let build_config = PyPIBuildConfig {
                        no_build_isolation: &non_isolated_packages,
                        no_build: &no_build,
                        no_binary: &no_binary,
                        index_strategy: index_strategy.as_ref(),
                        exclude_newer: exclude_newer.as_ref(),
                    };

                    let context_config = PyPIContextConfig {
                        uv_context: &uv_context,
                        pypi_indexes: pypi_indexes.as_ref(),
                        environment_variables: env_variables,
                    };

                    // Ignored pypi records
                    let names = ignored_pypi
                        .iter()
                        .map(to_uv_normalize)
                        .collect::<Result<Vec<_>, _>>()
                        .into_diagnostic()?;
                    PyPIEnvironmentUpdater::new(config, build_config, context_config)
                        .with_ignored_extraneous(names)
                        .update(&pixi_records, &pypi_records, &python_status)
                        .await
                }
                .with_context(|| {
                    format!(
                        "Failed to update PyPI packages for environment '{}'",
                        environment.name().fancy_display()
                    )
                })?;

                tracing::info!(
                    "Installed environment '{}' in {:?}",
                    environment.name().fancy_display(),
                    start.elapsed()
                );

                Ok(prefix)
            })
            .await
            .cloned()
    }

    fn locked_env(
        &self,
        environment: &Environment<'p>,
    ) -> Result<rattler_lock::Environment<'_>, UpdateError> {
        self.lock_file
            .environment(environment.name().as_str())
            .ok_or_else(|| UpdateError::LockFileMissingEnv(environment.name().clone()))
    }

    async fn conda_prefix(
        &self,
        environment: &Environment<'p>,
        reinstall_packages: Option<HashSet<PackageName>>,
        ignore_packages: Option<HashSet<PackageName>>,
    ) -> miette::Result<(Prefix, PythonStatus)> {
        // If we previously updated this environment, early out.
        let prefix_once_cell = self
            .updated_conda_prefixes
            .entry(environment.name().clone())
            .or_default()
            .clone();
        prefix_once_cell
            .get_or_try_init(async {
                // Create object to update the prefix
                let group = GroupedEnvironment::Environment(environment.clone());
                let platform = environment.best_platform();
                let virtual_packages = environment.virtual_packages(platform);

                let conda_prefix_updater = CondaPrefixUpdater::builder(
                    group,
                    platform,
                    virtual_packages
                        .into_iter()
                        .map(GenericVirtualPackage::from)
                        .collect(),
                    self.command_dispatcher.clone(),
                )
                .finish()?;

                // Get the locked environment from the lock-file.
                let locked_env = self.locked_env(environment)?;
                let packages = locked_env.packages(platform);
                let packages = if let Some(iter) = packages {
                    iter.collect_vec()
                } else {
                    Vec::new()
                };
                let records = locked_packages_to_pixi_records(packages)?;

                // Update the conda prefix
                let CondaPrefixUpdated {
                    prefix,
                    python_status,
                    ..
                } = conda_prefix_updater
                    .update(records, reinstall_packages, ignore_packages)
                    .await?;

                Ok((prefix.clone(), *python_status.clone()))
            })
            .await
            .map(|(prefix, python_status)| (prefix.clone(), python_status.clone()))
    }
}

/// The result of applying an InstallFilter over the lockfile for a given
/// environment, expressed as just package names.
#[derive(Default)]
pub struct PackageFilterNames {
    pub retained: Vec<String>,
    pub ignored: Vec<String>,
}

impl PackageFilterNames {
    pub fn new(
        filter: &InstallFilter,
        environment: rattler_lock::Environment<'_>,
        platform: Platform,
    ) -> Option<Self> {
        // Determine kept/ignored packages using the full install filter
        let subset = InstallSubset::new(
            &filter.skip_with_deps,
            &filter.skip_direct,
            &filter.target_packages,
        );
        let filtered = subset.filter(environment.packages(platform)).ok()?;

        // Map to names, dedupe and sort for stable output.
        let retained = filtered
            .install
            .into_iter()
            .map(|p| p.name().to_string())
            .unique()
            .sorted()
            .collect();
        let ignored = filtered
            .ignore
            .into_iter()
            .map(|p| p.name().to_string())
            .unique()
            .sorted()
            .collect();

        Some(Self { retained, ignored })
    }
}

fn locked_packages_to_pixi_records(
    conda_packages: Vec<LockedPackageRef<'_>>,
) -> Result<Vec<PixiRecord>, Report> {
    let pixi_records = conda_packages
        .into_iter()
        .filter_map(LockedPackageRef::as_conda)
        .cloned()
        .map(PixiRecord::try_from)
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;
    Ok(pixi_records)
}

pub struct UpdateContext<'p> {
    project: &'p Workspace,

    /// Repodata records from the lock-file. This contains the records that
    /// actually exist in the lock-file. If the lock-file is missing or
    /// partially missing then the data also won't exist in this field.
    locked_repodata_records: PerEnvironmentAndPlatform<'p, Arc<PixiRecordsByName>>,

    /// Repodata records from the lock-file grouped by solve-group.
    locked_grouped_repodata_records: PerGroupAndPlatform<'p, Arc<PixiRecordsByName>>,

    /// Pypi  records from the lock-file grouped by solve-group.
    locked_grouped_pypi_records: PerGroupAndPlatform<'p, Arc<PypiRecordsByName>>,

    /// Repodata records from the lock-file. This contains the records that
    /// actually exist in the lock-file. If the lock-file is missing or
    /// partially missing then the data also won't exist in this field.
    locked_pypi_records: PerEnvironmentAndPlatform<'p, Arc<PypiRecordsByName>>,

    /// Information about environments that are considered out of date. Only
    /// these environments are updated.
    outdated_envs: OutdatedEnvironments<'p>,

    /// Keeps track of all pending conda targets that are being solved. The
    /// mapping contains a [`BarrierCell`] that will eventually contain the
    /// solved records computed by another task. This allows tasks to wait
    /// for the records to be solved before proceeding.
    solved_repodata_records: PerEnvironmentAndPlatform<'p, Arc<BarrierCell<PixiRecordsByName>>>,

    /// Keeps track of all pending grouped conda targets that are being solved.
    grouped_solved_repodata_records: PerGroupAndPlatform<'p, Arc<BarrierCell<PixiRecordsByName>>>,

    /// Keeps track of all pending prefix updates. This only tracks the conda
    /// updates to a prefix, not whether the pypi packages have also been
    /// updated.
    instantiated_conda_prefixes: PerGroup<'p, Arc<BarrierCell<(Prefix, PythonStatus)>>>,

    /// Keeps track of all pending conda targets that are being solved. The
    /// mapping contains a [`BarrierCell`] that will eventually contain the
    /// solved records computed by another task. This allows tasks to wait
    /// for the records to be solved before proceeding.
    solved_pypi_records: PerEnvironmentAndPlatform<'p, Arc<BarrierCell<PypiRecordsByName>>>,

    /// Keeps track of all pending grouped pypi targets that are being solved.
    grouped_solved_pypi_records: PerGroupAndPlatform<'p, Arc<BarrierCell<PypiRecordsByName>>>,

    /// The package cache to use when instantiating prefixes.
    package_cache: PackageCache,

    /// The mapping client to use when fetching pypi mappings.
    mapping_client: MappingClient,

    /// A semaphore to limit the number of concurrent pypi solves.
    /// TODO(tim): we need this semaphore, to limit the number of concurrent
    ///     solves. This is a problem when using source dependencies
    pypi_solve_semaphore: Arc<Semaphore>,

    /// An io concurrency semaphore to limit the number of active filesystem
    /// operations.
    io_concurrency_limit: IoConcurrencyLimit,

    /// The command dispatcher
    command_dispatcher: CommandDispatcher,

    /// The input hash cache
    glob_hash_cache: GlobHashCache,

    /// Whether it is allowed to instantiate any prefix.
    no_install: bool,

    /// The progress bar where all the command dispatcher progress will be
    /// placed.
    dispatcher_progress_bar: ProgressBar,
}

impl<'p> UpdateContext<'p> {
    /// Returns a future that will resolve to the solved repodata records for
    /// the given environment group or `None` if the records do not exist
    /// and are also not in the process of being updated.
    pub(crate) fn get_latest_group_repodata_records(
        &self,
        group: &GroupedEnvironment<'p>,
        platform: Platform,
    ) -> Option<impl Future<Output = Arc<PixiRecordsByName>> + use<>> {
        // Check if there is a pending operation for this group and platform
        if let Some(pending_records) = self
            .grouped_solved_repodata_records
            .get(group)
            .and_then(|records| records.get(&platform))
            .cloned()
        {
            return Some((async move { pending_records.wait().await.clone() }).left_future());
        }

        // Otherwise read the records directly from the lock-file.
        let locked_records = self
            .locked_grouped_repodata_records
            .get(group)
            .and_then(|records| records.get(&platform))?
            .clone();

        Some(ready(locked_records).right_future())
    }

    /// Returns a future that will resolve to the solved pypi records for the
    /// given environment group or `None` if the records do not exist and
    /// are also not in the process of being updated.
    pub(crate) fn get_latest_group_pypi_records(
        &self,
        group: &GroupedEnvironment<'p>,
        platform: Platform,
    ) -> Option<impl Future<Output = Arc<PypiRecordsByName>> + use<>> {
        // Check if there is a pending operation for this group and platform
        if let Some(pending_records) = self
            .grouped_solved_pypi_records
            .get(group)
            .and_then(|records| records.get(&platform))
            .cloned()
        {
            return Some(async move { pending_records.wait().await.clone() });
        }

        None
    }

    /// Takes the latest repodata records for the given environment and
    /// platform. Returns `None` if neither the records exist nor are in the
    /// process of being updated.
    ///
    /// This function panics if the repodata records are still pending.
    pub(crate) fn take_latest_repodata_records(
        &mut self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<PixiRecordsByName> {
        self.solved_repodata_records
            .get_mut(environment)
            .and_then(|records| records.remove(&platform))
            .map(|cell| {
                Arc::into_inner(cell)
                    .expect("records must not be shared")
                    .into_inner()
                    .expect("records must be available")
            })
            .or_else(|| {
                self.locked_repodata_records
                    .get_mut(environment)
                    .and_then(|records| records.remove(&platform))
            })
            .map(|records| Arc::try_unwrap(records).unwrap_or_else(|arc| (*arc).clone()))
    }

    /// Takes the latest pypi records for the given environment and platform.
    /// Returns `None` if neither the records exist nor are in the process
    /// of being updated.
    ///
    /// This function panics if the repodata records are still pending.
    pub(crate) fn take_latest_pypi_records(
        &mut self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<PypiRecordsByName> {
        self.solved_pypi_records
            .get_mut(environment)
            .and_then(|records| records.remove(&platform))
            .map(|cell| {
                Arc::into_inner(cell)
                    .expect("records must not be shared")
                    .into_inner()
                    .expect("records must be available")
            })
            .or_else(|| {
                self.locked_pypi_records
                    .get_mut(environment)
                    .and_then(|records| records.remove(&platform))
            })
            .map(|records| Arc::try_unwrap(records).unwrap_or_else(|arc| (*arc).clone()))
    }

    /// Get a list of conda prefixes that have been updated.
    pub(crate) fn take_instantiated_conda_prefixes(
        &mut self,
    ) -> HashMap<EnvironmentName, (Prefix, PythonStatus)> {
        self.instantiated_conda_prefixes
            .drain()
            .filter_map(|(env, cell)| match env {
                GroupedEnvironment::Environment(env) => {
                    let prefix = Arc::into_inner(cell)
                        .expect("prefixes must not be shared")
                        .into_inner()
                        .expect("prefix must be available");
                    Some((env.name().clone(), (prefix.0.clone(), prefix.1.clone())))
                }
                _ => None,
            })
            .collect()
    }
}

/// If the project has any source dependencies, like `git` or `path`
/// dependencies. for pypi dependencies, we need to limit the solve to 1,
/// because of uv internals
fn determine_pypi_solve_permits(project: &Workspace) -> usize {
    // Get all environments
    let environments = project.environments();
    for environment in environments {
        for (_, req) in environment.pypi_dependencies(None).iter() {
            for dep in req {
                if dep.is_direct_dependency() {
                    return 1;
                }
            }
        }
    }
    // If no source dependencies are found, we can use the default concurrency
    project.config().max_concurrent_solves()
}

pub struct UpdateContextBuilder<'p> {
    /// The project
    project: &'p Workspace,

    /// The current lock-file.
    lock_file: LockFile,

    /// The environments that are considered outdated. These are the
    /// environments that will be updated in the lock-file. If this value is
    /// `None` it will be computed from the project and the lock-file.
    outdated_environments: Option<OutdatedEnvironments<'p>>,

    /// Defines if during the update-process it is allowed to create prefixes.
    /// This might be required to solve pypi dependencies because those require
    /// a python interpreter.
    no_install: bool,

    /// The package cache to use during the update process.
    package_cache: Option<PackageCache>,

    /// The mapping client to use for fetching pypi mappings.
    mapping_client: Option<MappingClient>,

    /// The io concurrency semaphore to use when updating environments
    io_concurrency_limit: Option<IoConcurrencyLimit>,

    /// A cache for computing input hashes
    glob_hash_cache: Option<GlobHashCache>,

    /// Set the command dispatcher to use for the update process.
    command_dispatcher: Option<CommandDispatcher>,
}

impl<'p> UpdateContextBuilder<'p> {
    pub(crate) fn with_glob_hash_cache(self, glob_hash_cache: GlobHashCache) -> Self {
        Self {
            glob_hash_cache: Some(glob_hash_cache),
            ..self
        }
    }

    /// The package cache to use during the update process. Prefixes might need
    /// to be instantiated to be able to solve pypi dependencies.
    pub(crate) fn with_package_cache(self, package_cache: PackageCache) -> Self {
        Self {
            package_cache: Some(package_cache),
            ..self
        }
    }

    /// Defines if during the update-process it is allowed to create prefixes.
    /// This might be required to solve pypi dependencies because those require
    /// a python interpreter.
    pub fn with_no_install(self, no_install: bool) -> Self {
        Self { no_install, ..self }
    }

    /// Sets the current lock-file that should be used to determine the
    /// previously locked packages.
    pub fn with_lock_file(self, lock_file: LockFile) -> Self {
        Self { lock_file, ..self }
    }

    /// Sets the command dispatcher to use for the update process.
    pub(crate) fn with_command_dispatcher(self, command_dispatcher: CommandDispatcher) -> Self {
        Self {
            command_dispatcher: Some(command_dispatcher),
            ..self
        }
    }

    /// Explicitly set the environments that are considered out-of-date. Only
    /// these environments will be updated during the update process.
    pub fn with_outdated_environments(
        self,
        outdated_environments: OutdatedEnvironments<'p>,
    ) -> Self {
        Self {
            outdated_environments: Some(outdated_environments),
            ..self
        }
    }

    /// Sets the io concurrency semaphore to use when updating environments.
    #[allow(unused)]
    pub fn with_io_concurrency_semaphore(self, io_concurrency_limit: IoConcurrencyLimit) -> Self {
        Self {
            io_concurrency_limit: Some(io_concurrency_limit),
            ..self
        }
    }

    /// Construct the context.
    pub async fn finish(self) -> miette::Result<UpdateContext<'p>> {
        let project = self.project;
        let package_cache = match self.package_cache {
            Some(package_cache) => package_cache,
            None => PackageCache::new(
                pixi_config::get_cache_dir()?.join(consts::CONDA_PACKAGE_CACHE_DIR),
            ),
        };
        let lock_file = self.lock_file;
        let glob_hash_cache = self.glob_hash_cache.unwrap_or_default();
        let outdated = match self.outdated_environments {
            Some(outdated) => outdated,
            None => {
                OutdatedEnvironments::from_workspace_and_lock_file(
                    project,
                    &lock_file,
                    glob_hash_cache.clone(),
                )
                .await
            }
        };

        // Extract the current conda records from the lock-file
        // TODO: Should we parallelize this? Measure please.
        let locked_repodata_records = project
            .environments()
            .into_iter()
            .flat_map(|env| {
                lock_file
                    .environment(env.name().as_str())
                    .into_iter()
                    .map(move |locked_env| {
                        locked_env
                            .conda_packages_by_platform()
                            .map(|(platform, records)| {
                                records
                                    .cloned()
                                    .map(PixiRecord::try_from)
                                    .collect::<Result<Vec<_>, _>>()
                                    .map(|records| {
                                        (platform, Arc::new(PixiRecordsByName::from_iter(records)))
                                    })
                            })
                            .collect::<Result<HashMap<_, _>, _>>()
                            .map(|records| (env.clone(), records))
                    })
            })
            .collect::<Result<HashMap<_, HashMap<_, _>>, _>>()
            .into_diagnostic()?;

        let locked_pypi_records = project
            .environments()
            .into_iter()
            .flat_map(|env| {
                lock_file
                    .environment(env.name().as_str())
                    .into_iter()
                    .map(move |locked_env| {
                        (
                            env.clone(),
                            locked_env
                                .pypi_packages_by_platform()
                                .map(|(platform, records)| {
                                    (
                                        platform,
                                        Arc::new(PypiRecordsByName::from_iter(records.map(
                                            |(data, env_data)| (data.clone(), env_data.clone()),
                                        ))),
                                    )
                                })
                                .collect(),
                        )
                    })
            })
            .collect::<HashMap<_, HashMap<_, _>>>();

        // Create a collection of all the [`GroupedEnvironments`] involved in the solve.
        let all_grouped_environments = project
            .environments()
            .into_iter()
            .map(GroupedEnvironment::from)
            .unique()
            .collect_vec();

        // For every grouped environment extract the data from the lock-file. If
        // multiple environments in a single solve-group have different versions for
        // a single package name than the record with the highest version is used.
        // This logic is implemented in `RepoDataRecordsByName::from_iter`. This can
        // happen if previously two environments did not share the same solve-group.
        let locked_grouped_repodata_records = all_grouped_environments
            .iter()
            .filter_map(|group| {
                // If any content of the environments in the group are outdated we need to
                // disregard the locked content.
                if group
                    .environments()
                    .any(|e| outdated.disregard_locked_content.should_disregard_conda(&e))
                {
                    return None;
                }

                let records = match group {
                    GroupedEnvironment::Environment(env) => {
                        locked_repodata_records.get(env)?.clone()
                    }
                    GroupedEnvironment::Group(group) => {
                        let mut by_platform = HashMap::new();
                        for env in group.environments() {
                            let Some(records) = locked_repodata_records.get(&env) else {
                                continue;
                            };

                            for (platform, records) in records.iter() {
                                by_platform
                                    .entry(*platform)
                                    .or_insert_with(Vec::new)
                                    .extend(records.records.iter().cloned());
                            }
                        }

                        by_platform
                            .into_iter()
                            .map(|(platform, records)| {
                                (platform, Arc::new(PixiRecordsByName::from_iter(records)))
                            })
                            .collect()
                    }
                };
                Some((group.clone(), records))
            })
            .collect();

        let locked_grouped_pypi_records = all_grouped_environments
            .iter()
            .filter_map(|group| {
                // If any content of the environments in the group are outdated we need to
                // disregard the locked content.
                if group
                    .environments()
                    .any(|e| outdated.disregard_locked_content.should_disregard_pypi(&e))
                {
                    return None;
                }

                let records = match group {
                    GroupedEnvironment::Environment(env) => locked_pypi_records.get(env)?.clone(),
                    GroupedEnvironment::Group(group) => {
                        let mut by_platform = HashMap::new();
                        for env in group.environments() {
                            let Some(records) = locked_pypi_records.get(&env) else {
                                continue;
                            };

                            for (platform, records) in records.iter() {
                                by_platform
                                    .entry(*platform)
                                    .or_insert_with(Vec::new)
                                    .extend(records.records.iter().cloned());
                            }
                        }

                        by_platform
                            .into_iter()
                            .map(|(platform, records)| {
                                (platform, Arc::new(PypiRecordsByName::from_iter(records)))
                            })
                            .collect()
                    }
                };
                Some((group.clone(), records))
            })
            .collect();

        let client = project.authenticated_client()?.clone();

        // Construct a command dispatcher that will be used to run the tasks.
        let multi_progress = global_multi_progress();
        let anchor_pb = multi_progress.add(ProgressBar::hidden());
        let command_dispatcher = match self.command_dispatcher {
            Some(dispatcher) => dispatcher,
            None => self
                .project
                .command_dispatcher_builder()?
                .with_reporter(pixi_reporters::TopLevelProgress::new(
                    global_multi_progress(),
                    anchor_pb.clone(),
                ))
                .finish(),
        };

        let mapping_client = self.mapping_client.unwrap_or_else(|| {
            MappingClient::builder(client)
                .with_concurrency_limit(project.concurrent_downloads_semaphore())
                .finish()
        });

        Ok(UpdateContext {
            project,

            locked_repodata_records,
            locked_grouped_repodata_records,
            locked_grouped_pypi_records,
            locked_pypi_records,
            outdated_envs: outdated,

            solved_repodata_records: HashMap::new(),
            instantiated_conda_prefixes: HashMap::new(),
            solved_pypi_records: HashMap::new(),
            grouped_solved_repodata_records: HashMap::new(),
            grouped_solved_pypi_records: HashMap::new(),

            mapping_client,
            package_cache,
            pypi_solve_semaphore: Arc::new(Semaphore::new(determine_pypi_solve_permits(project))),
            io_concurrency_limit: self.io_concurrency_limit.unwrap_or_default(),
            command_dispatcher,
            glob_hash_cache,
            dispatcher_progress_bar: anchor_pb,

            no_install: self.no_install,
        })
    }
}

impl<'p> UpdateContext<'p> {
    /// Construct a new builder for the update context.
    pub fn builder(project: &'p Workspace) -> UpdateContextBuilder<'p> {
        UpdateContextBuilder {
            project,
            lock_file: LockFile::default(),
            outdated_environments: None,
            no_install: true,
            package_cache: None,
            io_concurrency_limit: None,
            glob_hash_cache: None,
            mapping_client: None,
            command_dispatcher: None,
        }
    }

    pub async fn update(mut self) -> miette::Result<LockFileDerivedData<'p>> {
        let project = self.project;

        // Create a mapping that iterators over all outdated environments and their
        // platforms for both and pypi.
        let all_outdated_envs = itertools::chain(
            self.outdated_envs.conda.iter(),
            self.outdated_envs.pypi.iter(),
        )
        .fold(
            HashMap::<Environment<'_>, HashSet<Platform>>::new(),
            |mut acc, (env, platforms)| {
                acc.entry(env.clone())
                    .or_default()
                    .extend(platforms.iter().cloned());
                acc
            },
        );

        // This will keep track of all outstanding tasks that we need to wait for. All
        // tasks are added to this list after they are spawned. This function blocks
        // until all pending tasks have either completed or errored.
        let mut pending_futures = FuturesUnordered::new();

        // Spawn tasks for all the conda targets that are out of date.
        for (environment, platforms) in self.outdated_envs.conda.iter() {
            // Turn the platforms into an IndexSet, so we have a little control over the
            // order in which we solve the platforms. We want to solve the current
            // platform first, so we can start instantiating prefixes if we have to.
            let mut ordered_platforms = environment
                .platforms()
                .intersection(platforms)
                .copied()
                .collect::<IndexSet<_>>();
            if let Some(current_platform_index) =
                ordered_platforms.get_index_of(&environment.best_platform())
            {
                ordered_platforms.move_index(current_platform_index, 0);
            }

            // Determine the source of the solve information
            let source = GroupedEnvironment::from(environment.clone());

            // Determine the channel priority, if no channel priority is set we use the
            // default.
            let channel_priority = source.channel_priority()?.unwrap_or_default();

            for platform in ordered_platforms {
                // Is there an existing pending task to solve the group?
                if self
                    .grouped_solved_repodata_records
                    .get(&source)
                    .and_then(|platforms| platforms.get(&platform))
                    .is_some()
                {
                    // Yes, we can reuse the existing cell.
                    continue;
                }
                // No, we need to spawn a task to update for the entire solve group.
                let locked_group_records = self
                    .locked_grouped_repodata_records
                    .get(&source)
                    .and_then(|records| records.get(&platform))
                    .cloned()
                    .unwrap_or_default();

                // Spawn a task to solve the group.
                let group_solve_task = spawn_solve_conda_environment_task(
                    source.clone(),
                    locked_group_records,
                    self.mapping_client.clone(),
                    platform,
                    channel_priority,
                    self.command_dispatcher.clone(),
                )
                .map_err(Report::new)
                .boxed_local();

                // Store the task so we can poll it later.
                pending_futures.push(group_solve_task);

                // Create an entry that can be used by other tasks to wait for the result.
                let previous_cell = self
                    .grouped_solved_repodata_records
                    .entry(source.clone())
                    .or_default()
                    .insert(platform, Arc::default());
                assert!(
                    previous_cell.is_none(),
                    "a cell has already been added to update conda records"
                );
            }
        }

        // Spawn tasks to update the pypi packages.
        let uv_context = once_cell::sync::OnceCell::new();
        let mut pypi_conda_prefix_updaters = HashMap::new();
        for (environment, platform) in
            self.outdated_envs
                .pypi
                .iter()
                .flat_map(|(env, outdated_platforms)| {
                    let platforms_to_update = env
                        .platforms()
                        .intersection(outdated_platforms)
                        .cloned()
                        .collect_vec();
                    iter::once(env).cartesian_product(platforms_to_update)
                })
        {
            let group = GroupedEnvironment::from(environment.clone());

            // If the environment does not have any pypi dependencies we can skip it.
            if environment.pypi_dependencies(Some(platform)).is_empty() {
                continue;
            }

            // Solve all the pypi records in the solve group together.
            if self
                .grouped_solved_pypi_records
                .get(&group)
                .and_then(|records| records.get(&platform))
                .is_some()
            {
                // There is already a task to solve the pypi records for the group.
                continue;
            }

            // Get environment variables from the activation
            let project_variables = self.project.env_vars().clone();
            // Construct a future that will resolve when we have the repodata available
            let repodata_solve_platform_future = self
                .get_latest_group_repodata_records(&group, platform)
                .ok_or_else(|| make_unsupported_pypi_platform_error(environment, true))?;
            // Construct an optional future that will resolve for building the pypi sources,
            // the error is delayed to raise at the time when building the sources.
            let repodata_building_env = self
                .get_latest_group_repodata_records(&group, environment.best_platform())
                .ok_or_else(|| make_unsupported_pypi_platform_error(environment, false));

            // Creates an object to initiate an update at a later point. Make sure to only
            // create a single entry if we are solving for multiple platforms.
            let conda_prefix_updater =
                match pypi_conda_prefix_updaters.entry(environment.name().clone()) {
                    Entry::Vacant(entry) => {
                        let prefix_platform = environment.best_platform();
                        let conda_prefix_updater = CondaPrefixUpdater::builder(
                            group.clone(),
                            prefix_platform,
                            environment
                                .virtual_packages(prefix_platform)
                                .into_iter()
                                .map(GenericVirtualPackage::from)
                                .collect(),
                            self.command_dispatcher.clone(),
                        )
                        .finish()?;
                        entry.insert(conda_prefix_updater).clone()
                    }
                    Entry::Occupied(entry) => entry.get().clone(),
                };

            let uv_context = uv_context
                .get_or_try_init(|| UvResolutionContext::from_workspace(project))?
                .clone();

            let locked_group_records = self
                .locked_grouped_pypi_records
                .get(&group)
                .and_then(|records| records.get(&platform))
                .cloned()
                .unwrap_or_default();

            // Spawn a task to solve the pypi environment
            let pypi_solve_future = spawn_solve_pypi_task(
                uv_context,
                group.clone(),
                environment.clone(),
                project_variables,
                platform,
                repodata_solve_platform_future,
                repodata_building_env,
                conda_prefix_updater,
                self.pypi_solve_semaphore.clone(),
                project.root().to_path_buf(),
                locked_group_records,
                self.no_install,
            );

            pending_futures.push(pypi_solve_future.boxed_local());

            let previous_cell = self
                .grouped_solved_pypi_records
                .entry(group)
                .or_default()
                .insert(platform, Arc::default());
            assert!(
                previous_cell.is_none(),
                "a cell has already been added to update pypi records"
            );
        }

        // Iterate over all outdated environments and their platforms and extract the
        // corresponding records from them.
        for (environment, platform) in
            all_outdated_envs
                .iter()
                .flat_map(|(env, outdated_platforms)| {
                    let platforms_to_update = outdated_platforms
                        .intersection(&env.platforms())
                        .cloned()
                        .collect_vec();
                    iter::once(env.clone()).cartesian_product(platforms_to_update)
                })
        {
            let grouped_environment = GroupedEnvironment::from(environment.clone());

            // Get futures that will resolve when the conda and pypi records become
            // available.
            let grouped_repodata_records = self
                .get_latest_group_repodata_records(&grouped_environment, platform)
                .expect("conda records should be available now or in the future");
            let grouped_pypi_records = self
                .get_latest_group_pypi_records(&grouped_environment, platform)
                .map(Either::Left)
                .unwrap_or_else(|| Either::Right(ready(Arc::default())));

            // Spawn a task to extract a subset of the resolution.
            let extract_resolution_task = spawn_extract_environment_task(
                environment.clone(),
                platform,
                grouped_repodata_records,
                grouped_pypi_records,
            );
            pending_futures.push(extract_resolution_task.boxed_local());

            // Create a cell that will be used to store the result of the extraction.
            let previous_cell = self
                .solved_repodata_records
                .entry(environment.clone())
                .or_default()
                .insert(platform, Arc::default());
            assert!(
                previous_cell.is_none(),
                "a cell has already been added to update conda records"
            );

            let previous_cell = self
                .solved_pypi_records
                .entry(environment.clone())
                .or_default()
                .insert(platform, Arc::default());
            assert!(
                previous_cell.is_none(),
                "a cell has already been added to update pypi records"
            );
        }

        let top_level_progress = global_multi_progress()
            .insert_before(&self.dispatcher_progress_bar, ProgressBar::hidden());
        top_level_progress.set_style(indicatif::ProgressStyle::default_bar()
            .template("{spinner:.cyan} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {pos:>4}/{len:4} {wide_msg:.dim}")
            .expect("should be able to set style")
            .progress_chars("━━╾─"));
        top_level_progress.enable_steady_tick(Duration::from_millis(50));
        top_level_progress.set_prefix("updating lock-file");
        top_level_progress.set_length(pending_futures.len() as u64);

        // Iterate over all the futures we spawned and wait for them to complete.
        //
        // The spawned futures each result either in an error or in a `TaskResult`. The
        // `TaskResult` contains the result of the task. The results are stored into
        // [`BarrierCell`]s which allows other tasks to respond to the data becoming
        // available.
        //
        // A loop on the main task is used versus individually spawning all tasks for
        // two reasons:
        //
        // 1. This provides some control over when data is polled and broadcasted to
        //    other tasks. No data is broadcasted until we start polling futures here.
        //    This reduces the risk of race-conditions where data has already been
        //    broadcasted before a task subscribes to it.
        // 2. The futures stored in `pending_futures` do not necessarily have to be
        //    `'static`. Which makes them easier to work with.
        while let Some(result) = pending_futures.next().await {
            top_level_progress.inc(1);
            match result? {
                TaskResult::CondaGroupSolved(group_name, platform, records, duration) => {
                    let group = GroupedEnvironment::from_name(project, &group_name)
                        .expect("group should exist");

                    self.grouped_solved_repodata_records
                        .get_mut(&group)
                        .expect("the entry for this environment should exist")
                        .get_mut(&platform)
                        .expect("the entry for this platform should exist")
                        .set(Arc::new(records))
                        .expect("records should not be solved twice");

                    match group_name {
                        GroupedEnvironmentName::Group(_) => {
                            tracing::info!(
                                "resolved conda environment for solve group '{}' '{}' in {}",
                                group_name.fancy_display(),
                                consts::PLATFORM_STYLE.apply_to(platform),
                                humantime::format_duration(duration)
                            );
                        }
                        GroupedEnvironmentName::Environment(env_name) => {
                            tracing::info!(
                                "resolved conda environment for environment '{}' '{}' in {}",
                                env_name.fancy_display(),
                                consts::PLATFORM_STYLE.apply_to(platform),
                                humantime::format_duration(duration)
                            );
                        }
                    }
                }
                TaskResult::PypiGroupSolved(
                    group_name,
                    platform,
                    records,
                    duration,
                    conda_prefix,
                ) => {
                    let group = GroupedEnvironment::from_name(project, &group_name)
                        .expect("group should exist");

                    self.grouped_solved_pypi_records
                        .get_mut(&group)
                        .expect("the entry for this environment should exist")
                        .get_mut(&platform)
                        .expect("the entry for this platform should exist")
                        .set(Arc::new(records))
                        .expect("records should not be solved twice");

                    match group_name {
                        GroupedEnvironmentName::Group(_) => {
                            tracing::info!(
                                "resolved pypi packages for solve group '{}' '{}' in {}",
                                group_name.fancy_display(),
                                consts::PLATFORM_STYLE.apply_to(platform),
                                humantime::format_duration(duration),
                            );
                        }
                        GroupedEnvironmentName::Environment(env_name) => {
                            tracing::info!(
                                "resolved pypi packages for environment '{}' '{}' in {}",
                                env_name.fancy_display(),
                                consts::PLATFORM_STYLE.apply_to(platform),
                                humantime::format_duration(duration),
                            );
                        }
                    }

                    if let Some(conda_prefix) = conda_prefix {
                        let group = GroupedEnvironment::from_name(project, &conda_prefix.group)
                            .expect("grouped environment should exist");

                        self.instantiated_conda_prefixes
                            .get_mut(&group)
                            .expect("the entry for this environment should exists")
                            .set(Arc::new((conda_prefix.prefix, *conda_prefix.python_status)))
                            .expect("prefix should not be instantiated twice");

                        tracing::info!(
                            "updated conda packages in the '{}' prefix in {}",
                            group.name().fancy_display(),
                            humantime::format_duration(duration)
                        );
                    }
                }
                TaskResult::ExtractedRecordsSubset(
                    environment,
                    platform,
                    repodata_records,
                    pypi_records,
                ) => {
                    let environment = project
                        .environment(&environment)
                        .expect("environment should exist");

                    self.solved_pypi_records
                        .get_mut(&environment)
                        .expect("the entry for this environment should exist")
                        .get_mut(&platform)
                        .expect("the entry for this platform should exist")
                        .set(pypi_records)
                        .expect("records should not be solved twice");

                    self.solved_repodata_records
                        .get_mut(&environment)
                        .expect("the entry for this environment should exist")
                        .get_mut(&platform)
                        .expect("the entry for this platform should exist")
                        .set(repodata_records)
                        .expect("records should not be solved twice");

                    let group = GroupedEnvironment::from(environment.clone());
                    if matches!(group, GroupedEnvironment::Group(_)) {
                        tracing::info!(
                            "extracted subset of records for '{}' '{}' from the '{}' group",
                            environment.name().fancy_display(),
                            consts::PLATFORM_STYLE.apply_to(platform),
                            group.name().fancy_display(),
                        );
                    }
                }
            }
        }

        // Construct a new lock-file containing all the updated or old records.
        let mut builder = LockFile::builder();

        // Iterate over all environments and add their records to the lock-file.
        for environment in project.environments() {
            let environment_name = environment.name().to_string();
            let grouped_env = GroupedEnvironment::from(environment.clone());

            let channel_config = project.channel_config();
            let channels: Vec<String> = grouped_env
                .channels()
                .into_iter()
                .cloned()
                .map(|channel| {
                    channel
                        .into_base_url(&channel_config)
                        .map(|ch| ch.to_string())
                })
                .try_collect()
                .into_diagnostic()?;

            builder.set_channels(&environment_name, channels);
            builder.set_options(
                &environment_name,
                rattler_lock::SolveOptions {
                    strategy: grouped_env.solve_strategy(),
                    channel_priority: grouped_env
                        .channel_priority()
                        .unwrap_or_default()
                        .unwrap_or_default()
                        .into(),
                    exclude_newer: grouped_env.exclude_newer(),
                },
            );

            let mut has_pypi_records = false;
            for platform in environment.platforms() {
                if let Some(records) = self.take_latest_repodata_records(&environment, platform) {
                    for record in records.into_inner() {
                        builder.add_conda_package(&environment_name, platform, record.into());
                    }
                }
                if let Some(records) = self.take_latest_pypi_records(&environment, platform) {
                    for (pkg_data, pkg_env_data) in records.into_inner() {
                        builder.add_pypi_package(
                            &environment_name,
                            platform,
                            pkg_data,
                            pkg_env_data,
                        );
                        has_pypi_records = true;
                    }
                }
            }

            // Store the indexes that were used to solve the environment. But only if there
            // are pypi packages.
            if has_pypi_records {
                builder.set_pypi_indexes(&environment_name, grouped_env.pypi_options().into());
            }
        }

        // Store the lock file
        let lock_file = builder.finish();
        top_level_progress.finish_and_clear();

        Ok(LockFileDerivedData {
            workspace: project,
            lock_file,
            updated_conda_prefixes: self
                .take_instantiated_conda_prefixes()
                .into_iter()
                .map(|(key, value)| (key, Arc::new(async_once_cell::OnceCell::new_with(value))))
                .collect(),
            package_cache: self.package_cache,
            updated_pypi_prefixes: Default::default(),
            uv_context,
            io_concurrency_limit: self.io_concurrency_limit,
            command_dispatcher: self.command_dispatcher,
            glob_hash_cache: self.glob_hash_cache,
        })
    }
}

/// Constructs an error that indicates that the current platform cannot solve
/// pypi dependencies because there is no python interpreter available for the
/// current platform.
fn make_unsupported_pypi_platform_error(
    environment: &Environment<'_>,
    top_level_error: bool,
) -> Report {
    let grouped_environment = GroupedEnvironment::from(environment.clone());
    let current_platform = environment.best_platform();
    let platforms = environment.platforms();

    let mut diag = if top_level_error {
        MietteDiagnostic::new(format!(
            "Unable to solve pypi dependencies for the {} {} — there is no compatible Python interpreter for '{}'",
            grouped_environment.name().fancy_display(),
            match &grouped_environment {
                GroupedEnvironment::Group(_) => "solve group",
                GroupedEnvironment::Environment(_) => "environment",
            },
            consts::PLATFORM_STYLE.apply_to(current_platform),
        ))
    } else {
        MietteDiagnostic::new(format!(
            "there is no compatible Python interpreter for '{}'",
            consts::PLATFORM_STYLE.apply_to(current_platform),
        ))
    };

    let help_message = if !platforms.contains(&current_platform) {
        // State 1: The current platform is not in the `platforms` list
        format!(
            "Try: {}",
            consts::TASK_STYLE.apply_to(format!("pixi workspace platform add {current_platform}")),
        )
    } else {
        // State 2: Python is not in the dependencies.
        format!("Try: {}", consts::TASK_STYLE.apply_to("pixi add python"))
    };

    diag.help = Some(help_message);

    Report::new(diag)
}

/// Represents data that is sent back from a task. This is used to communicate
/// the result of a task back to the main task which will forward the
/// information to other tasks waiting for results.
pub enum TaskResult {
    /// The conda dependencies for a grouped environment have been solved.
    CondaGroupSolved(
        GroupedEnvironmentName,
        Platform,
        PixiRecordsByName,
        Duration,
    ),

    /// The pypi dependencies for a grouped environment have been solved.
    PypiGroupSolved(
        GroupedEnvironmentName,
        Platform,
        PypiRecordsByName,
        Duration,
        Option<CondaPrefixUpdated>,
    ),

    /// The records for a specific environment have been extracted from a
    /// grouped solve.
    ExtractedRecordsSubset(
        EnvironmentName,
        Platform,
        Arc<PixiRecordsByName>,
        Arc<PypiRecordsByName>,
    ),
}

/// A task that solves the conda dependencies for a given environment.
#[allow(clippy::too_many_arguments)]
async fn spawn_solve_conda_environment_task(
    group: GroupedEnvironment<'_>,
    existing_repodata_records: Arc<PixiRecordsByName>,
    mapping_client: MappingClient,
    platform: Platform,
    channel_priority: ChannelPriority,
    command_dispatcher: CommandDispatcher,
) -> Result<TaskResult, SolveCondaEnvironmentError> {
    // Get the dependencies for this platform
    let dependencies = group.combined_dependencies(Some(platform));

    // Get solve options
    let exclude_newer = group.exclude_newer();
    let strategy = group.solve_strategy();

    // Get the environment name
    let group_name = group.name();

    // Early out if there are no dependencies to solve.
    if dependencies.is_empty() {
        return Ok(TaskResult::CondaGroupSolved(
            group_name,
            platform,
            PixiRecordsByName::default(),
            Duration::default(),
        ));
    }

    // Get the virtual packages for this platform
    let virtual_packages = group.virtual_packages(platform);

    // The list of channels and platforms we need for this task
    let channels = group.channels().into_iter().cloned().collect_vec();

    // Whether there are pypi dependencies, and we should fetch purls.
    let has_pypi_dependencies = group.has_pypi_dependencies();

    // Whether we should use custom mapping location
    let pypi_name_mapping_location = group
        .workspace()
        .pypi_name_mapping_source()
        .map_err(|err| SolveCondaEnvironmentError::PypiMappingFailed(err.into()))?
        .clone();

    // Get the channel configuration
    let channel_config = group.workspace().channel_config();

    // Resolve the channel URLs for the channels we need.
    let channels = channels
        .iter()
        .map(|c| c.clone().into_base_url(&channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Determine the build variants
    let variants = group.workspace().variants(platform);

    let start = Instant::now();

    // Solve the environment using the command dispatcher.
    let mut records = command_dispatcher
        .solve_pixi_environment(PixiEnvironmentSpec {
            name: Some(group_name.to_string()),
            dependencies,
            constraints: Default::default(),
            installed: existing_repodata_records.records.clone(),
            build_environment: BuildEnvironment::simple(platform, virtual_packages),
            channels,
            strategy,
            channel_priority: channel_priority.into(),
            exclude_newer,
            channel_config,
            variants: Some(variants),
            enabled_protocols: Default::default(),
        })
        .await
        .map_err(|source| SolveCondaEnvironmentError::SolveFailed {
            environment_name: group_name.clone(),
            platform,
            source,
        })?;

    // Add purl's for the conda packages that are also available as pypi packages if
    // we need them.
    if has_pypi_dependencies {
        // TODO: Bring back the pypi mapping reporter
        mapping_client
            .amend_purls(
                &pypi_name_mapping_location,
                records.iter_mut().filter_map(PixiRecord::as_binary_mut),
                None,
            )
            .await
            .map_err(|err| SolveCondaEnvironmentError::PypiMappingFailed(err.into()))?;
    }

    // Turn the records into a map by name
    let records_by_name = PixiRecordsByName::from(records);

    let end = Instant::now();

    Ok(TaskResult::CondaGroupSolved(
        group_name,
        platform,
        records_by_name,
        end - start,
    ))
}

/// Distill the repodata that is applicable for the given `environment` from the
/// repodata of an entire solve group.
async fn spawn_extract_environment_task(
    environment: Environment<'_>,
    platform: Platform,
    grouped_repodata_records: impl Future<Output = Arc<PixiRecordsByName>>,
    grouped_pypi_records: impl Future<Output = Arc<PypiRecordsByName>>,
) -> miette::Result<TaskResult> {
    let group = GroupedEnvironment::from(environment.clone());

    // Await the records from the group
    let (grouped_repodata_records, grouped_pypi_records) =
        tokio::join!(grouped_repodata_records, grouped_pypi_records);

    // If the group is just the environment on its own we can immediately return the
    // records.
    if let GroupedEnvironment::Environment(_) = group {
        return Ok(TaskResult::ExtractedRecordsSubset(
            environment.name().clone(),
            platform,
            grouped_repodata_records,
            grouped_pypi_records,
        ));
    }

    // Convert all the conda records to package identifiers.
    let conda_package_identifiers = grouped_repodata_records.by_pypi_name().into_diagnostic()?;

    #[derive(Clone, Eq, PartialEq, Hash)]
    enum PackageName {
        Conda(rattler_conda_types::PackageName),
        Pypi((uv_normalize::PackageName, Option<ExtraName>)),
    }

    enum PackageRecord<'a> {
        Conda(&'a PixiRecord),
        Pypi((&'a PypiRecord, Option<ExtraName>)),
    }

    // Determine the conda packages we need.
    let conda_package_names = environment
        .combined_dependencies(Some(platform))
        .names()
        .cloned()
        .map(PackageName::Conda)
        .collect::<Vec<_>>();

    // Determine the pypi packages we need.
    let pypi_dependencies = environment.pypi_dependencies(Some(platform));
    let has_pypi_dependencies = !pypi_dependencies.is_empty();
    let mut pypi_package_names = HashSet::new();
    for (name, reqs) in pypi_dependencies {
        let name = name.as_normalized().clone();
        let uv_name = to_uv_normalize(&name).into_diagnostic()?;
        for req in reqs {
            for extra in req.extras().iter() {
                pypi_package_names.insert(PackageName::Pypi((
                    uv_name.clone(),
                    Some(to_uv_extra_name(extra).into_diagnostic()?),
                )));
            }
        }
        pypi_package_names.insert(PackageName::Pypi((uv_name, None)));
    }

    // Compute the Pypi marker environment. Only do this if we have pypi
    // dependencies.
    let marker_environment = if has_pypi_dependencies {
        grouped_repodata_records
            .python_interpreter_record()
            .and_then(|record| determine_marker_environment(platform, &record.package_record).ok())
    } else {
        None
    };

    // Construct a queue of packages that we need to check.
    let mut queue = itertools::chain(conda_package_names, pypi_package_names).collect::<Vec<_>>();
    let mut queued_names = queue.iter().cloned().collect::<HashSet<_>>();

    let mut pixi_records = Vec::new();
    let mut pypi_records = HashMap::new();
    while let Some(package) = queue.pop() {
        let record = match package {
            PackageName::Conda(name) => grouped_repodata_records
                .by_name(&name)
                .map(PackageRecord::Conda),
            PackageName::Pypi((name, extra)) => {
                let pep_name = to_normalize(&name).into_diagnostic()?;
                if let Some(found_record) = grouped_pypi_records.by_name(&pep_name) {
                    Some(PackageRecord::Pypi((found_record, extra)))
                } else if let Some((_, _, found_record)) = conda_package_identifiers.get(&name) {
                    Some(PackageRecord::Conda(found_record))
                } else {
                    None
                }
            }
        };

        let Some(record) = record else {
            // If this happens we are missing a dependency from the grouped environment. We
            // currently just ignore this.
            continue;
        };

        match record {
            PackageRecord::Conda(record) => {
                // Find all dependencies in the record and add them to the queue.
                for dependency in record.package_record().depends.iter() {
                    let dependency_name =
                        PackageName::Conda(rattler_conda_types::PackageName::new_unchecked(
                            dependency.split_once(' ').unwrap_or((dependency, "")).0,
                        ));
                    if queued_names.insert(dependency_name.clone()) {
                        queue.push(dependency_name);
                    }
                }

                // Store the record itself as part of the subset
                pixi_records.push(record);
            }
            PackageRecord::Pypi((record, extra)) => {
                // Evaluate all dependencies
                let extras = extra
                    .map(|extra| Ok::<_, ConversionError>(vec![to_extra_name(&extra)?]))
                    .transpose()
                    .into_diagnostic()?
                    .unwrap_or_default();

                for req in record.0.requires_dist.iter() {
                    // Evaluate the marker environment with the given extras
                    if let Some(marker_env) = &marker_environment {
                        // let marker_str = marker_env.to_string();
                        let pep_marker = to_marker_environment(marker_env).into_diagnostic()?;

                        if !req.evaluate_markers(&pep_marker, &extras) {
                            continue;
                        }
                    }
                    let uv_name = to_uv_normalize(&req.name).into_diagnostic()?;

                    // Add the package to the queue
                    for extra in req.extras.iter() {
                        let extra_name = to_uv_extra_name(extra).into_diagnostic()?;
                        if queued_names.insert(PackageName::Pypi((
                            uv_name.clone(),
                            Some(extra_name.clone()),
                        ))) {
                            queue.push(PackageName::Pypi((
                                uv_name.clone(),
                                Some(extra_name.clone()),
                            )));
                        }
                    }

                    // Also add the dependency without any extras
                    queue.push(PackageName::Pypi((uv_name, None)));
                }

                // Insert the record if it is not already present
                pypi_records.entry(record.0.name.clone()).or_insert(record);
            }
        }
    }

    Ok(TaskResult::ExtractedRecordsSubset(
        environment.name().clone(),
        platform,
        Arc::new(PixiRecordsByName::from_iter(
            pixi_records.into_iter().cloned(),
        )),
        Arc::new(PypiRecordsByName::from_iter(
            pypi_records.into_values().cloned(),
        )),
    ))
}

/// A task that solves the pypi dependencies for a given environment.
#[allow(clippy::too_many_arguments)]
async fn spawn_solve_pypi_task<'p>(
    resolution_context: UvResolutionContext,
    grouped_environment: GroupedEnvironment<'p>,
    environment: Environment<'p>,
    project_variables: HashMap<EnvironmentName, EnvironmentVars>,
    platform: Platform,
    repodata_solve_records: impl Future<Output = Arc<PixiRecordsByName>>,
    repodata_building_records: miette::Result<impl Future<Output = Arc<PixiRecordsByName>>>,
    prefix_task: CondaPrefixUpdater,
    semaphore: Arc<Semaphore>,
    project_root: PathBuf,
    locked_pypi_packages: Arc<PypiRecordsByName>,
    disallow_install_conda_prefix: bool,
) -> miette::Result<TaskResult> {
    // Get the Pypi dependencies for this environment
    let dependencies = grouped_environment.pypi_dependencies(Some(platform));
    if dependencies.is_empty() {
        return Ok(TaskResult::PypiGroupSolved(
            grouped_environment.name().clone(),
            platform,
            PypiRecordsByName::default(),
            Duration::from_millis(0),
            None,
        ));
    }

    let exclude_newer = grouped_environment.exclude_newer();

    // Get the system requirements for this environment
    let system_requirements = grouped_environment.system_requirements();

    // Wait until the conda records and prefix are available.
    let (repodata_records, repodata_building_records) = match repodata_building_records {
        Ok(repodata_building_records) => {
            let (repodata_records, repodata_building_records, _guard) = tokio::join!(
                repodata_solve_records,
                repodata_building_records,
                semaphore.acquire_owned()
            );
            (repodata_records, Ok(repodata_building_records))
        }
        Err(err) => {
            let (repodata_records, _guard) =
                tokio::join!(repodata_solve_records, semaphore.acquire_owned());
            (repodata_records, Err(err))
        }
    };

    let environment_name = grouped_environment.name().clone();

    let pixi_solve_records = &repodata_records.records;
    let locked_pypi_records = &locked_pypi_packages.records;

    let pypi_options = environment.pypi_options();
    let (pypi_packages, duration, prefix_task_result) = async move {
        let pb = SolveProgressBar::new(
            global_multi_progress().add(ProgressBar::hidden()),
            platform,
            environment_name.clone(),
        );
        pb.start();

        let start = Instant::now();

        let dependencies: Vec<(uv_normalize::PackageName, IndexSet<_>)> = dependencies
            .into_iter()
            .map(|(name, requirement)| Ok((to_uv_normalize(name.as_normalized())?, requirement)))
            .collect::<Result<_, ConversionError>>()
            .into_diagnostic()?;

        let requirements = IndexMap::from_iter(dependencies);

        let (records, prefix_task_result) = lock_file::resolve_pypi(
            resolution_context,
            &pypi_options,
            requirements,
            system_requirements,
            pixi_solve_records,
            locked_pypi_records,
            platform,
            &pb.pb,
            &project_root,
            prefix_task,
            repodata_building_records,
            project_variables,
            environment,
            disallow_install_conda_prefix,
            exclude_newer,
        )
        .await
        .with_context(|| {
            format!(
                "failed to solve the pypi requirements of environment '{}' for platform '{}'",
                environment_name.fancy_display(),
                consts::PLATFORM_STYLE.apply_to(platform)
            )
        })?;
        let end = Instant::now();

        pb.finish();

        Ok::<(_, _, _), miette::Report>((
            PypiRecordsByName::from_iter(records),
            end - start,
            prefix_task_result,
        ))
    }
    .instrument(tracing::info_span!(
        "resolve_pypi",
        group = %grouped_environment.name().as_str(),
        platform = %platform
    ))
    .await?;

    Ok(TaskResult::PypiGroupSolved(
        grouped_environment.name().clone(),
        platform,
        pypi_packages,
        duration,
        prefix_task_result,
    ))
}
