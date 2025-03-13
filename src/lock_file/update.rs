use std::{
    cmp::PartialEq,
    collections::{HashMap, HashSet},
    future::{ready, Future},
    iter,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    environment::{CondaPrefixUpdated, CondaPrefixUpdaterBuilder},
    workspace::{get_activated_environment_variables, EnvironmentVars},
};
use barrier_cell::BarrierCell;
use fancy_display::FancyDisplay;
use futures::{stream::FuturesUnordered, FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::{Diagnostic, IntoDiagnostic, MietteDiagnostic, Report, WrapErr};
use pixi_build_frontend::ToolContext;
use pixi_consts::consts;
use pixi_manifest::{ChannelPriority, EnvironmentName, FeaturesExt};
use pixi_progress::global_multi_progress;
use pixi_record::{ParseLockFileError, PixiRecord};
use pixi_uv_conversions::{
    to_extra_name, to_marker_environment, to_normalize, to_uv_extra_name, to_uv_normalize,
    ConversionError,
};
use pypi_mapping::{self};
use pypi_modifiers::pypi_marker_env::determine_marker_environment;
use rattler::package_cache::PackageCache;
use rattler_conda_types::{Arch, MatchSpec, ParseStrictness, Platform};
use rattler_lock::{
    LockFile, ParseCondaLockError, PypiIndexes, PypiPackageData, PypiPackageEnvironmentData,
};
use rattler_repodata_gateway::{Gateway, RepoData};
use reqwest_middleware::ClientWithMiddleware;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::Instrument;
use uv_normalize::ExtraName;

use super::{
    outdated::OutdatedEnvironments, utils::IoConcurrencyLimit, CondaPrefixUpdater,
    PixiRecordsByName, PypiRecordsByName, UvResolutionContext,
};
use crate::{
    activation::CurrentEnvVarBehavior,
    build::{BuildContext, GlobHashCache, SourceCheckoutReporter},
    environment::{
        self, read_environment_file, write_environment_file, EnvironmentFile, LockFileUsage,
        LockedEnvironmentHash, PerEnvironmentAndPlatform, PerGroup, PerGroupAndPlatform,
        PythonStatus,
    },
    lock_file::{
        self,
        records_by_name::HasNameVersion,
        reporter::{CondaMetadataProgress, GatewayProgressReporter, SolveProgressBar},
        virtual_packages::validate_system_meets_environment_requirements,
        PypiRecord,
    },
    prefix::Prefix,
    repodata::Repodata,
    workspace::{
        grouped_environment::{GroupedEnvironment, GroupedEnvironmentName},
        Environment, HasWorkspaceRef,
    },
    Workspace,
};

impl Workspace {
    /// Ensures that the lock-file is up-to-date with the project.
    ///
    /// This function will return a `LockFileDerivedData` struct that contains the
    /// lock-file and any potential derived data that was computed as part of this
    /// function. The derived data might be usable by other functions to avoid
    /// recomputing the same data.
    ///
    /// This function starts by checking if the lock-file is up-to-date. If it is
    /// not up-to-date it will construct a task graph of all the work that needs to
    /// be done to update the lock-file. The tasks are awaited in a specific order
    /// to make sure that we can start instantiating prefixes as soon as possible.
    pub async fn update_lock_file(
        &self,
        options: UpdateLockFileOptions,
    ) -> miette::Result<LockFileDerivedData<'_>> {
        let lock_file = self.load_lock_file().await?;
        let package_cache =
            PackageCache::new(pixi_config::get_cache_dir()?.join(consts::CONDA_PACKAGE_CACHE_DIR));
        let glob_hash_cache = GlobHashCache::default();

        // should we check the lock-file in the first place?
        if !options.lock_file_usage.should_check_if_out_of_date() {
            tracing::info!("skipping check if lock-file is up-to-date");

            return Ok(LockFileDerivedData {
                workspace: self,
                lock_file,
                package_cache,
                updated_conda_prefixes: Default::default(),
                updated_pypi_prefixes: Default::default(),
                uv_context: None,
                io_concurrency_limit: IoConcurrencyLimit::default(),
                build_context: BuildContext::from_workspace(self)?,
                glob_hash_cache,
            });
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
            return Ok(LockFileDerivedData {
                workspace: self,
                lock_file,
                package_cache,
                updated_conda_prefixes: Default::default(),
                updated_pypi_prefixes: Default::default(),
                uv_context: None,
                io_concurrency_limit: IoConcurrencyLimit::default(),
                build_context: BuildContext::from_workspace(self)?,
                glob_hash_cache,
            });
        }

        // If the lock-file is out of date, but we're not allowed to update it, we
        // should exit.
        if !options.lock_file_usage.allows_lock_file_updates() {
            miette::bail!("lock-file not up-to-date with the workspace");
        }

        // Construct an update context and perform the actual update.
        let lock_file_derived_data = UpdateContext::builder(self)
            .with_package_cache(package_cache)
            .with_no_install(options.no_install)
            .with_outdated_environments(outdated)
            .with_lock_file(lock_file)
            .with_glob_hash_cache(glob_hash_cache)
            .finish()
            .await?
            .update()
            .await?;

        // Write the lock-file to disk
        lock_file_derived_data.write_to_disk()?;

        Ok(lock_file_derived_data)
    }

    /// Loads the lockfile for the workspace or returns `Lockfile::default` if none
    /// could be found.
    pub async fn load_lock_file(&self) -> miette::Result<LockFile> {
        let lock_file_path = self.lock_file_path();
        if lock_file_path.is_file() {
            // Spawn a background task because loading the file might be IO bound.
            tokio::task::spawn_blocking(move || {
            LockFile::from_path(&lock_file_path)
                .map_err(|err| match err {
                    ParseCondaLockError::IncompatibleVersion{ lock_file_version, max_supported_version} => {
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

/// A struct that holds the lock-file and any potential derived data that was
/// computed when calling `update_lock_file`.
pub struct LockFileDerivedData<'p> {
    pub workspace: &'p Workspace,

    /// The lock-file
    pub lock_file: LockFile,

    /// The package cache
    pub package_cache: PackageCache,

    /// A list of prefixes that are up-to-date with the latest conda packages.
    pub updated_conda_prefixes: HashMap<EnvironmentName, (Prefix, PythonStatus)>,

    /// A list of prefixes that have been updated while resolving all
    /// dependencies.
    pub updated_pypi_prefixes: HashMap<EnvironmentName, Prefix>,

    /// The cached uv context
    pub uv_context: Option<UvResolutionContext>,

    /// The IO concurrency semaphore to use when updating environments
    pub io_concurrency_limit: IoConcurrencyLimit,

    /// The build context that was used to create the lock-file
    pub build_context: BuildContext,

    /// An object that caches input hashes
    pub glob_hash_cache: GlobHashCache,
}

/// The mode to use when updating a prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub(crate) fn write_to_disk(&self) -> miette::Result<()> {
        let lock_file_path = self.workspace.lock_file_path();
        self.lock_file
            .to_path(&lock_file_path)
            .into_diagnostic()
            .context("failed to write lock-file to disk")
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
        &mut self,
        environment: &Environment<'p>,
        update_mode: UpdateMode,
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
        let prefix = self.update_prefix(environment).await?;

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
        &mut self,
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
                tracing::debug!("Lock file contains source packages: ignore lock file hash and update the prefix");
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
    async fn update_prefix(&mut self, environment: &Environment<'p>) -> miette::Result<Prefix> {
        // If we previously updated this environment, early out.
        if let Some(prefix) = self.updated_pypi_prefixes.get(environment.name()) {
            return Ok(prefix.clone());
        }

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

        tracing::info!("Updating prefix: '{}'", environment.dir().display());
        // Get the prefix with the conda packages installed.
        let platform = environment.best_platform();
        let (prefix, python_status) = self.conda_prefix(environment).await?;
        let pixi_records = self
            .pixi_records(environment, platform)
            .into_diagnostic()?
            .unwrap_or_default();
        let pypi_records = self
            .pypi_records(environment, platform)
            .into_diagnostic()?
            .unwrap_or_default();

        // No `uv` support for WASM right now
        if platform.arch() == Some(Arch::Wasm32) {
            return Ok(prefix);
        }

        let uv_context = match &self.uv_context {
            None => {
                let context = UvResolutionContext::from_workspace(self.workspace)?;
                self.uv_context = Some(context.clone());
                context
            }
            Some(context) => context.clone(),
        };

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

        // Update the prefix with Pypi records
        environment::update_prefix_pypi(
            environment.name(),
            &prefix,
            platform,
            &pixi_records,
            &pypi_records,
            &python_status,
            &environment.system_requirements(),
            &uv_context,
            self.pypi_indexes(environment).into_diagnostic()?.as_ref(),
            env_variables,
            self.workspace.root(),
            environment.best_platform(),
            non_isolated_packages,
            &no_build,
        )
        .await
        .with_context(|| {
            format!(
                "Failed to update PyPI packages for environment '{}'",
                environment.name().fancy_display()
            )
        })?;

        // Store that we updated the environment, so we won't have to do it again.
        self.updated_pypi_prefixes
            .insert(environment.name().clone(), prefix.clone());

        Ok(prefix)
    }

    fn pypi_records(
        &self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Result<Option<Vec<(PypiPackageData, PypiPackageEnvironmentData)>>, UpdateError> {
        let locked_env = self
            .lock_file
            .environment(environment.name().as_str())
            .ok_or_else(|| UpdateError::LockFileMissingEnv(environment.name().clone()))?;

        let packages = locked_env.pypi_packages(platform);
        Ok(packages.map(|iter| {
            iter.map(|(data, env_data)| (data.clone(), env_data.clone()))
                .collect()
        }))
    }

    fn pypi_indexes(
        &self,
        environment: &Environment<'p>,
    ) -> Result<Option<PypiIndexes>, UpdateError> {
        let locked_env = self
            .lock_file
            .environment(environment.name().as_str())
            .ok_or_else(|| UpdateError::LockFileMissingEnv(environment.name().clone()))?;
        Ok(locked_env.pypi_indexes().cloned())
    }

    fn pixi_records(
        &self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Result<Option<Vec<PixiRecord>>, UpdateError> {
        let locked_env = self
            .lock_file
            .environment(environment.name().as_str())
            .ok_or_else(|| UpdateError::LockFileMissingEnv(environment.name().clone()))?;

        Ok(locked_env
            .conda_packages(platform)
            .map(|iter| {
                iter.cloned()
                    .map(PixiRecord::try_from)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?)
    }

    async fn conda_prefix(
        &mut self,
        environment: &Environment<'p>,
    ) -> miette::Result<(Prefix, PythonStatus)> {
        // If we previously updated this environment, early out.
        if let Some((prefix, python_status)) = self.updated_conda_prefixes.get(environment.name()) {
            return Ok((prefix.clone(), python_status.clone()));
        }

        // Create object to update the prefix
        let group = GroupedEnvironment::Environment(environment.clone());
        let platform = environment.best_platform();

        let conda_prefix_updater = CondaPrefixUpdaterBuilder::new(
            group,
            platform,
            self.package_cache.clone(),
            self.io_concurrency_limit.clone(),
            self.build_context.clone(),
        )
        .build()?;

        // Get the locked environment from the lock-file.
        let records = self
            .pixi_records(environment, platform)
            .into_diagnostic()?
            .unwrap_or_default();
        // Update the conda prefix
        let CondaPrefixUpdated {
            prefix,
            python_status,
            ..
        } = conda_prefix_updater.update(records).await?;

        // Store that we updated the environment, so we won't have to do it again.
        self.updated_conda_prefixes.insert(
            environment.name().clone(),
            (prefix.clone(), *python_status.clone()),
        );

        Ok((prefix.clone(), *python_status.clone()))
    }
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

    /// A semaphore to limit the number of concurrent solves.
    conda_solve_semaphore: Arc<Semaphore>,

    /// A semaphore to limit the number of concurrent pypi solves.
    /// TODO(tim): we need this semaphore, to limit the number of concurrent
    ///     solves. This is a problem when using source dependencies
    pypi_solve_semaphore: Arc<Semaphore>,

    /// An io concurrency semaphore to limit the number of active filesystem
    /// operations.
    io_concurrency_limit: IoConcurrencyLimit,

    /// The build context to use for building source packages
    build_context: BuildContext,

    /// The input hash cache
    glob_hash_cache: GlobHashCache,

    /// Whether it is allowed to instantiate any prefix.
    no_install: bool,
}

impl<'p> UpdateContext<'p> {
    /// Returns a future that will resolve to the solved repodata records for
    /// the given environment group or `None` if the records do not exist
    /// and are also not in the process of being updated.
    pub(crate) fn get_latest_group_repodata_records(
        &self,
        group: &GroupedEnvironment<'p>,
        platform: Platform,
    ) -> Option<impl Future<Output = Arc<PixiRecordsByName>>> {
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
    ) -> Option<impl Future<Output = Arc<PypiRecordsByName>>> {
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

    /// The maximum number of concurrent solves that are allowed to run. If this
    /// value is `None` a heuristic is used based on the number of cores
    /// available from the system.
    max_concurrent_solves: usize,

    /// The io concurrency semaphore to use when updating environments
    io_concurrency_limit: Option<IoConcurrencyLimit>,

    /// A cache for computing input hashes
    glob_hash_cache: Option<GlobHashCache>,
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
    pub(crate) fn with_no_install(self, no_install: bool) -> Self {
        Self { no_install, ..self }
    }

    /// Sets the current lock-file that should be used to determine the
    /// previously locked packages.
    pub(crate) fn with_lock_file(self, lock_file: LockFile) -> Self {
        Self { lock_file, ..self }
    }

    /// Explicitly set the environments that are considered out-of-date. Only
    /// these environments will be updated during the update process.
    pub(crate) fn with_outdated_environments(
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
    pub(crate) async fn finish(self) -> miette::Result<UpdateContext<'p>> {
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

        let gateway = project.repodata_gateway()?.clone();
        let client = project.authenticated_client()?.clone();

        // tool context
        let tool_context = ToolContext::builder()
            .with_gateway(gateway)
            .with_client(client)
            .build();

        let build_context =
            BuildContext::from_workspace(project)?.with_tool_context(Arc::new(tool_context));

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

            package_cache,
            conda_solve_semaphore: Arc::new(Semaphore::new(self.max_concurrent_solves)),
            pypi_solve_semaphore: Arc::new(Semaphore::new(determine_pypi_solve_permits(project))),
            io_concurrency_limit: self.io_concurrency_limit.unwrap_or_default(),
            build_context,
            glob_hash_cache,

            no_install: self.no_install,
        })
    }
}

impl<'p> UpdateContext<'p> {
    /// Construct a new builder for the update context.
    pub(crate) fn builder(project: &'p Workspace) -> UpdateContextBuilder<'p> {
        UpdateContextBuilder {
            project,
            lock_file: LockFile::default(),
            outdated_environments: None,
            no_install: true,
            package_cache: None,
            max_concurrent_solves: project.config().max_concurrent_solves(),
            io_concurrency_limit: None,
            glob_hash_cache: None,
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
            let mut ordered_platforms = platforms.iter().copied().collect::<IndexSet<_>>();
            if let Some(current_platform_index) =
                ordered_platforms.get_index_of(&environment.best_platform())
            {
                ordered_platforms.move_index(current_platform_index, 0);
            }

            // Determine the source of the solve information
            let source = GroupedEnvironment::from(environment.clone());

            // Determine the channel priority, if no channel priority is set we use the
            // default.
            let channel_priority = source
                .channel_priority()
                .into_diagnostic()?
                .unwrap_or_default();

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
                    project.repodata_gateway()?.clone(),
                    platform,
                    self.conda_solve_semaphore.clone(),
                    project.authenticated_client()?.clone(),
                    channel_priority,
                    self.build_context.clone(),
                )
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
        let mut uv_context = None;
        for (environment, platform) in self
            .outdated_envs
            .pypi
            .iter()
            .flat_map(|(env, platforms)| platforms.iter().map(move |p| (env, *p)))
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
                .ok_or_else(|| make_unsupported_pypi_platform_error(environment))?;
            let repodata_current_platform = self
                .get_latest_group_repodata_records(&group, environment.best_platform())
                .ok_or_else(|| make_unsupported_pypi_platform_error(environment))?;

            // Creates an object to initiate an update at a later point
            let conda_prefix_updater = CondaPrefixUpdaterBuilder::new(
                group.clone(),
                environment.best_platform(),
                self.package_cache.clone(),
                self.io_concurrency_limit.clone(),
                self.build_context.clone(),
            )
            .build()?;

            // Get the uv context
            let uv_context = match uv_context.as_ref() {
                None => uv_context
                    .insert(UvResolutionContext::from_workspace(project)?)
                    .clone(),
                Some(context) => context.clone(),
            };

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
                repodata_current_platform,
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

        // Iteratate over all outdated environments and their platforms and extract the
        // corresponding records from them.
        for (environment, platform) in all_outdated_envs.iter().flat_map(|(env, platforms)| {
            iter::once(env.clone()).cartesian_product(platforms.iter().cloned())
        }) {
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

        let top_level_progress =
            global_multi_progress().add(ProgressBar::new(pending_futures.len() as u64));
        top_level_progress.set_style(indicatif::ProgressStyle::default_bar()
            .template("{spinner:.cyan} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {pos:>4}/{len:4} {wide_msg:.dim}")
            .expect("should be able to set style")
            .progress_chars("━━╾─"));
        top_level_progress.enable_steady_tick(Duration::from_millis(50));
        top_level_progress.set_prefix("updating lock-file");

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
            updated_conda_prefixes: self.take_instantiated_conda_prefixes(),
            package_cache: self.package_cache,
            updated_pypi_prefixes: HashMap::default(),
            uv_context,
            io_concurrency_limit: self.io_concurrency_limit,
            build_context: self.build_context,
            glob_hash_cache: self.glob_hash_cache,
        })
    }
}

/// Constructs an error that indicates that the current platform cannot solve
/// pypi dependencies because there is no python interpreter available for the
/// current platform.
fn make_unsupported_pypi_platform_error(environment: &Environment<'_>) -> miette::Report {
    let grouped_environment = GroupedEnvironment::from(environment.clone());

    // Construct a diagnostic that explains that the current platform is not
    // supported.
    let mut diag = MietteDiagnostic::new(format!("Unable to solve pypi dependencies for the {} {} because no compatible python interpreter can be installed for the current platform", grouped_environment.name().fancy_display(), match &grouped_environment {
        GroupedEnvironment::Group(_) => "solve group",
        GroupedEnvironment::Environment(_) => "environment"
    }));

    diag.help = Some("Try converting your [pypi-dependencies] to conda [dependencies]".to_string());

    miette::Report::new(diag)
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
    repodata_gateway: Gateway,
    platform: Platform,
    concurrency_semaphore: Arc<Semaphore>,
    client: ClientWithMiddleware,
    channel_priority: ChannelPriority,
    build_context: BuildContext,
) -> miette::Result<TaskResult> {
    // Get the dependencies for this platform
    let dependencies = group.combined_dependencies(Some(platform));

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
    let pypi_name_mapping_location = group.workspace().pypi_name_mapping_source()?.clone();

    // Get the channel configuration
    let channel_config = group.workspace().channel_config();

    // A root progress bar for the task. It is used to attach sub-progress bars to,
    // that doesn't need to be split up between multiple platforms.
    let root_pb = global_multi_progress().add(ProgressBar::hidden());

    tokio::spawn(
        async move {
            // Acquire a permit before we are allowed to solve the environment.
            let _permit = concurrency_semaphore
                .acquire()
                .await
                .expect("the semaphore is never closed");

            let pb = Arc::new(SolveProgressBar::new(
                global_multi_progress().add(ProgressBar::hidden()),
                platform,
                group_name.clone(),
            ));
            pb.start();
            pb.set_message("loading repodata");

            let start = Instant::now();

            // Convert the dependencies into match specs and source dependencies
            let (source_specs, match_specs): (Vec<_>, Vec<_>) = dependencies
                .into_specs()
                .partition_map(
                    |(name, constraint)| match constraint.into_source_or_binary() {
                        Either::Left(source) => Either::Left((name, source)),
                        Either::Right(binary) => {
                            let spec = binary
                                .try_into_nameless_match_spec(&channel_config)
                                .expect("failed to convert channel from spec");
                            Either::Right(MatchSpec::from_nameless(spec, Some(name)))
                        }
                    },
                );

            // Collect metadata from all source packages
            let channel_urls = channels
                .iter()
                .map(|c| c.clone().into_base_url(&channel_config))
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;

            let mut metadata_progress = None;
            let mut source_progress = None;
            let mut source_match_specs = Vec::new();
            let source_futures = FuturesUnordered::new();
            for (build_id, (name, source_spec)) in source_specs.iter().enumerate() {
                // Create a metadata reporter if it doesn't exist yet.
                let metadata_reporter = metadata_progress.get_or_insert_with(|| {
                    Arc::new(CondaMetadataProgress::new(
                        &pb.pb,
                        source_specs.len() as u64,
                    ))
                });
                let source_reporter = source_progress.get_or_insert_with(|| {
                    Arc::new(SourceCheckoutReporter::new(
                        root_pb.clone(),
                        global_multi_progress(),
                    ))
                });

                source_futures.push(
                    build_context
                        .extract_source_metadata(
                            source_spec,
                            &channel_urls,
                            platform,
                            virtual_packages.clone(),
                            platform,
                            virtual_packages.clone(),
                            metadata_reporter.clone(),
                            Some(source_reporter.clone()),
                            build_id,
                        )
                        .map_err(|e| {
                            Report::new(e).wrap_err(format!(
                                "failed to extract metadata for '{}'",
                                name.as_source()
                            ))
                        }),
                );

                // Add a dependency to the source package itself.
                // TODO: We also need to make sure that only the source package is used when
                //  passing these packages to the gateway.
                source_match_specs.push(MatchSpec {
                    name: Some(name.clone()),
                    ..MatchSpec::default()
                })
            }
            let source_repodata: Vec<_> = source_futures.try_collect().await?;

            // Extract transitive requirements from the requirements of the source packages
            let mut query_match_specs = match_specs.clone();
            for source_repodata in source_repodata
                .iter()
                .flat_map(|r| r.records.iter())
                .flat_map(|r| &r.package_record.depends)
            {
                if let Ok(spec) = MatchSpec::from_str(source_repodata, ParseStrictness::Lenient) {
                    query_match_specs.push(spec);
                }
            }

            // Extract the repo data records needed to solve the environment.
            let fetch_repodata_start = Instant::now();
            let available_packages = repodata_gateway
                .query(
                    channels
                        .into_iter()
                        .map(|c| c.into_channel(&channel_config))
                        .collect::<Result<Vec<_>, _>>()
                        .into_diagnostic()?,
                    [platform, Platform::NoArch],
                    query_match_specs,
                )
                .recursive(true)
                .with_reporter(GatewayProgressReporter::new(pb.clone()))
                .await
                .into_diagnostic()?;
            let total_records = available_packages.iter().map(RepoData::len).sum::<usize>();
            tracing::info!(
                "fetched {total_records} records in {:?}",
                fetch_repodata_start.elapsed()
            );

            // Solve conda packages
            pb.reset_style();
            pb.set_message("resolving conda");

            let mut all_specs = match_specs;
            all_specs.extend(source_match_specs);

            // Update the locked records by filtering out any source records. These will be
            // locked again every time.
            let source_package_records: HashSet<rattler_conda_types::PackageName> = source_repodata
                .iter()
                .flat_map(|record| record.records.iter())
                .map(|record| record.package_record.name.clone())
                .collect();
            let locked_records = existing_repodata_records
                .records
                .iter()
                .filter_map(|record| {
                    let record = record.as_binary()?;
                    if source_package_records.contains(record.name()) {
                        None
                    } else {
                        Some(record.clone())
                    }
                })
                .collect();

            let mut records = lock_file::resolve_conda(
                all_specs,
                virtual_packages,
                locked_records,
                available_packages,
                source_repodata,
                channel_priority,
            )
            .await
            .with_context(|| {
                format!(
                    "failed to solve the conda requirements of '{}' '{}'",
                    group_name.fancy_display(),
                    consts::PLATFORM_STYLE.apply_to(platform)
                )
            })?;

            // Add purl's for the conda packages that are also available as pypi packages if
            // we need them.
            if has_pypi_dependencies {
                pb.set_message("mapping conda to pypi packages");
                pypi_mapping::amend_pypi_purls(
                    client,
                    &pypi_name_mapping_location,
                    records.iter_mut().filter_map(PixiRecord::as_binary_mut),
                    Some(pb.purl_amend_reporter()),
                )
                .await?;
            }

            // Turn the records into a map by name
            let records_by_name = PixiRecordsByName::from(records);

            let end = Instant::now();

            // Finish the progress bar
            pb.finish();

            Ok(TaskResult::CondaGroupSolved(
                group_name,
                platform,
                records_by_name,
                end - start,
            ))
        }
        .instrument(tracing::info_span!(
            "resolve_conda",
            group = %group.name().as_str(),
            platform = %platform
        )),
    )
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })
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
    repodata_current_records: impl Future<Output = Arc<PixiRecordsByName>>,
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

    // Get the system requirements for this environment
    let system_requirements = grouped_environment.system_requirements();

    // Wait until the conda records and prefix are available.
    let (repodata_records, repodata_current_records, _guard) = tokio::join!(
        repodata_solve_records,
        repodata_current_records,
        semaphore.acquire_owned()
    );

    let environment_name = grouped_environment.name().clone();

    let pypi_name_mapping_location = grouped_environment.workspace().pypi_name_mapping_source()?;

    let mut pixi_solve_records = repodata_records.records.clone();
    let locked_pypi_records = locked_pypi_packages.records.clone();

    pypi_mapping::amend_pypi_purls(
        environment.workspace().authenticated_client()?.clone(),
        pypi_name_mapping_location,
        pixi_solve_records
            .iter_mut()
            .filter_map(PixiRecord::as_binary_mut),
        None,
    )
    .await?;

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
            &pixi_solve_records,
            &locked_pypi_records,
            platform,
            &pb.pb,
            &project_root,
            prefix_task,
            repodata_current_records,
            project_variables,
            environment,
            disallow_install_conda_prefix,
        )
        .await
        .with_context(|| {
            format!(
                "failed to solve the pypi requirements of '{}' '{}'",
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
