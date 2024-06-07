use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write,
    future::{ready, Future},
    iter,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures::{future::Either, stream::FuturesUnordered, FutureExt, StreamExt, TryFutureExt};
use indexmap::IndexSet;
use indicatif::{HumanBytes, ProgressBar, ProgressState};
use itertools::Itertools;
use miette::{IntoDiagnostic, LabeledSpan, MietteDiagnostic, WrapErr};
use parking_lot::Mutex;
use rattler::package_cache::PackageCache;
use rattler_conda_types::{Arch, MatchSpec, Platform, RepoDataRecord};
use rattler_lock::{LockFile, PypiPackageData, PypiPackageEnvironmentData};
use rattler_repodata_gateway::{Gateway, RepoData};
use tokio::sync::Semaphore;
use tracing::Instrument;
use url::Url;
use uv_normalize::ExtraName;

use crate::{
    config, consts,
    environment::{
        self, LockFileUsage, PerEnvironmentAndPlatform, PerGroup, PerGroupAndPlatform, PythonStatus,
    },
    load_lock_file,
    lock_file::{
        self, update, OutdatedEnvironments, PypiRecord, PypiRecordsByName, RepoDataRecordsByName,
        UvResolutionContext,
    },
    prefix::Prefix,
    progress::global_multi_progress,
    project::{
        grouped_environment::{GroupedEnvironment, GroupedEnvironmentName},
        has_features::HasFeatures,
        Environment,
    },
    pypi_mapping::{self, Reporter},
    pypi_marker_env::determine_marker_environment,
    pypi_tags::is_python_record,
    utils::BarrierCell,
    EnvironmentName, Project,
};

impl Project {
    /// Ensures that the lock-file is up-to-date with the project information.
    ///
    /// Returns the lock-file and any potential derived data that was computed
    /// as part of this operation.
    pub async fn up_to_date_lock_file(
        &self,
        options: UpdateLockFileOptions,
    ) -> miette::Result<LockFileDerivedData<'_>> {
        update::ensure_up_to_date_lock_file(self, options).await
    }
}

/// Options to pass to [`Project::up_to_date_lock_file`].
#[derive(Default)]
pub struct UpdateLockFileOptions {
    /// Defines what to do if the lock-file is out of date
    pub lock_file_usage: LockFileUsage,

    /// Don't install anything to disk.
    pub no_install: bool,

    /// The maximum number of concurrent solves that are allowed to run. If this
    /// value is None a heuristic is used based on the number of cores
    /// available from the system.
    pub max_concurrent_solves: Option<usize>,
}

/// A struct that holds the lock-file and any potential derived data that was
/// computed when calling `ensure_up_to_date_lock_file`.
pub struct LockFileDerivedData<'p> {
    pub project: &'p Project,

    /// The lock-file
    pub lock_file: LockFile,

    /// The package cache
    pub package_cache: PackageCache,

    /// A list of prefixes that are up-to-date with the latest conda packages.
    pub updated_conda_prefixes: HashMap<Environment<'p>, (Prefix, PythonStatus)>,

    /// A list of prefixes that have been updated while resolving all
    /// dependencies.
    pub updated_pypi_prefixes: HashMap<Environment<'p>, Prefix>,

    /// The cached uv context
    pub uv_context: Option<UvResolutionContext>,
}

impl<'p> LockFileDerivedData<'p> {
    /// Write the lock-file to disk.
    pub fn write_to_disk(&self) -> miette::Result<()> {
        let lock_file_path = self.project.lock_file_path();
        self.lock_file
            .to_path(&lock_file_path)
            .into_diagnostic()
            .context("failed to write lock-file to disk")
    }

    /// Returns the up-to-date prefix for the given environment.
    pub async fn prefix(&mut self, environment: &Environment<'p>) -> miette::Result<Prefix> {
        if let Some(prefix) = self.updated_pypi_prefixes.get(environment) {
            return Ok(prefix.clone());
        }

        // Get the prefix with the conda packages installed.
        let platform = environment.best_platform();
        let (prefix, python_status) = self.conda_prefix(environment).await?;
        let repodata_records = self
            .repodata_records(environment, platform)
            .unwrap_or_default();
        let pypi_records = self.pypi_records(environment, platform).unwrap_or_default();

        // No `uv` support for WASM right now
        if platform.arch() == Some(Arch::Wasm32) {
            return Ok(prefix);
        }

        let uv_context = match &self.uv_context {
            None => {
                let context = UvResolutionContext::from_project(self.project)?;
                self.uv_context = Some(context.clone());
                context
            }
            Some(context) => context.clone(),
        };

        let env_variables = environment
            .project()
            .get_env_variables(environment, false)
            .await?;
        // Update the prefix with Pypi records
        environment::update_prefix_pypi(
            environment.name(),
            &prefix,
            platform,
            &repodata_records,
            &pypi_records,
            &python_status,
            &environment.system_requirements(),
            &uv_context,
            &environment.pypi_options(),
            env_variables,
            self.project.root(),
            environment.best_platform(),
        )
        .await
        .with_context(|| "error updating pypi prefix")?;

        // Store that we updated the environment, so we won't have to do it again.
        self.updated_pypi_prefixes
            .insert(environment.clone(), prefix.clone());

        Ok(prefix)
    }

    fn pypi_records(
        &self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<Vec<(PypiPackageData, PypiPackageEnvironmentData)>> {
        let locked_env = self
            .lock_file
            .environment(environment.name().as_str())
            .expect("the lock-file should be up-to-date so it should also include the environment");
        locked_env.pypi_packages_for_platform(platform)
    }

    fn repodata_records(
        &self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<Vec<RepoDataRecord>> {
        let locked_env = self
            .lock_file
            .environment(environment.name().as_str())
            .expect("the lock-file should be up-to-date so it should also include the environment");
        locked_env.conda_repodata_records_for_platform(platform).expect("since the lock-file is up to date we should be able to extract the repodata records from it")
    }

    async fn conda_prefix(
        &mut self,
        environment: &Environment<'p>,
    ) -> miette::Result<(Prefix, PythonStatus)> {
        // If we previously updated this environment, early out.
        if let Some((prefix, python_status)) = self.updated_conda_prefixes.get(environment) {
            return Ok((prefix.clone(), python_status.clone()));
        }

        let prefix = Prefix::new(environment.dir());
        let platform = environment.best_platform();

        // Determine the currently installed packages.
        let installed_packages = prefix
            .find_installed_packages(None)
            .await
            .with_context(|| {
                format!(
                    "failed to determine the currently installed packages for '{}'",
                    environment.name(),
                )
            })?;

        // Get the locked environment from the lock-file.
        let records = self
            .repodata_records(environment, platform)
            .unwrap_or_default();

        // Update the prefix with conda packages.
        let has_existing_packages = !installed_packages.is_empty();
        let env_name = GroupedEnvironmentName::Environment(environment.name().clone());
        let python_status = environment::update_prefix_conda(
            &prefix,
            self.package_cache.clone(),
            environment.project().authenticated_client().clone(),
            installed_packages,
            records,
            platform,
            &format!(
                "{} environment '{}'",
                if has_existing_packages {
                    "updating"
                } else {
                    "creating"
                },
                env_name.fancy_display()
            ),
            "",
        )
        .await?;

        // Store that we updated the environment, so we won't have to do it again.
        self.updated_conda_prefixes
            .insert(environment.clone(), (prefix.clone(), python_status.clone()));

        Ok((prefix, python_status))
    }
}

pub struct UpdateContext<'p> {
    project: &'p Project,

    /// Repodata records from the lock-file. This contains the records that
    /// actually exist in the lock-file. If the lock-file is missing or
    /// partially missing then the data also won't exist in this field.
    locked_repodata_records: PerEnvironmentAndPlatform<'p, Arc<RepoDataRecordsByName>>,

    /// Repodata records from the lock-file grouped by solve-group.
    locked_grouped_repodata_records: PerGroupAndPlatform<'p, Arc<RepoDataRecordsByName>>,

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
    solved_repodata_records:
        PerEnvironmentAndPlatform<'p, Arc<BarrierCell<Arc<RepoDataRecordsByName>>>>,

    /// Keeps track of all pending grouped conda targets that are being solved.
    grouped_solved_repodata_records:
        PerGroupAndPlatform<'p, Arc<BarrierCell<Arc<RepoDataRecordsByName>>>>,

    /// Keeps track of all pending prefix updates. This only tracks the conda
    /// updates to a prefix, not whether the pypi packages have also been
    /// updated.
    instantiated_conda_prefixes: PerGroup<'p, Arc<BarrierCell<(Prefix, PythonStatus)>>>,

    /// Keeps track of all pending conda targets that are being solved. The
    /// mapping contains a [`BarrierCell`] that will eventually contain the
    /// solved records computed by another task. This allows tasks to wait
    /// for the records to be solved before proceeding.
    solved_pypi_records: PerEnvironmentAndPlatform<'p, Arc<BarrierCell<Arc<PypiRecordsByName>>>>,

    /// Keeps track of all pending grouped pypi targets that are being solved.
    grouped_solved_pypi_records: PerGroupAndPlatform<'p, Arc<BarrierCell<Arc<PypiRecordsByName>>>>,

    /// The package cache to use when instantiating prefixes.
    package_cache: PackageCache,

    /// A semaphore to limit the number of concurrent solves.
    conda_solve_semaphore: Arc<Semaphore>,

    /// A semaphore to limit the number of concurrent pypi solves.
    /// TODO(tim): we need this semaphore, to limit the number of concurrent
    ///     solves. This is a problem when using source dependencies
    pypi_solve_semaphore: Arc<Semaphore>,

    /// Whether it is allowed to instantiate any prefix.
    no_install: bool,
}

impl<'p> UpdateContext<'p> {
    /// Returns a future that will resolve to the solved repodata records for
    /// the given environment group or `None` if the records do not exist
    /// and are also not in the process of being updated.
    pub fn get_latest_group_repodata_records(
        &self,
        group: &GroupedEnvironment<'p>,
        platform: Platform,
    ) -> Option<impl Future<Output = Arc<RepoDataRecordsByName>>> {
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
    pub fn get_latest_group_pypi_records(
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
    pub fn take_latest_repodata_records(
        &mut self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<RepoDataRecordsByName> {
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
    pub fn take_latest_pypi_records(
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
    pub fn take_instantiated_conda_prefixes(
        &mut self,
    ) -> HashMap<Environment<'p>, (Prefix, PythonStatus)> {
        self.instantiated_conda_prefixes
            .drain()
            .filter_map(|(env, cell)| match env {
                GroupedEnvironment::Environment(env) => {
                    let prefix = Arc::into_inner(cell)
                        .expect("prefixes must not be shared")
                        .into_inner()
                        .expect("prefix must be available");
                    Some((env, prefix))
                }
                _ => None,
            })
            .collect()
    }

    /// Returns a future that will resolve to the solved repodata records for
    /// the given environment or `None` if no task was spawned to
    /// instantiate the prefix.
    pub fn get_conda_prefix(
        &self,
        environment: &GroupedEnvironment<'p>,
    ) -> Option<impl Future<Output = (Prefix, PythonStatus)>> {
        let cell = self.instantiated_conda_prefixes.get(environment)?.clone();
        Some(async move { cell.wait().await.clone() })
    }
}

/// Returns the default number of concurrent solves.
fn default_max_concurrent_solves() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// If the project has any source dependencies, like `git` or `path`
/// dependencies. for pypi dependencies, we need to limit the solve to 1,
/// because of uv internals
fn determine_pypi_solve_permits(project: &Project) -> usize {
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

    default_max_concurrent_solves()
}

/// Ensures that the lock-file is up-to-date with the project.
///
/// This function will return a [`LockFileDerivedData`] struct that contains the
/// lock-file and any potential derived data that was computed as part of this
/// function. The derived data might be usable by other functions to avoid
/// recomputing the same data.
///
/// This function starts by checking if the lock-file is up-to-date. If it is
/// not up-to-date it will construct a task graph of all the work that needs to
/// be done to update the lock-file. The tasks are awaited in a specific order
/// to make sure that we can start instantiating prefixes as soon as possible.
pub async fn ensure_up_to_date_lock_file(
    project: &Project,
    options: UpdateLockFileOptions,
) -> miette::Result<LockFileDerivedData<'_>> {
    let lock_file = load_lock_file(project).await?;
    let package_cache = PackageCache::new(config::get_cache_dir()?.join("pkgs"));

    // should we check the lock-file in the first place?
    if !options.lock_file_usage.should_check_if_out_of_date() {
        tracing::info!("skipping check if lock-file is up-to-date");

        return Ok(LockFileDerivedData {
            project,
            lock_file,
            package_cache,
            updated_conda_prefixes: Default::default(),
            updated_pypi_prefixes: Default::default(),
            uv_context: None,
        });
    }

    // Check which environments are out of date.
    let outdated = OutdatedEnvironments::from_project_and_lock_file(project, &lock_file);
    if outdated.is_empty() {
        tracing::info!("the lock-file is up-to-date");

        // If no-environment is outdated we can return early.
        return Ok(LockFileDerivedData {
            project,
            lock_file,
            package_cache,
            updated_conda_prefixes: Default::default(),
            updated_pypi_prefixes: Default::default(),
            uv_context: None,
        });
    }

    // If the lock-file is out of date, but we're not allowed to update it, we
    // should exit.
    if !options.lock_file_usage.allows_lock_file_updates() {
        miette::bail!("lock-file not up-to-date with the project");
    }

    let max_concurrent_solves = options
        .max_concurrent_solves
        .unwrap_or_else(default_max_concurrent_solves);

    // Construct an update context and perform the actual update.
    let lock_file_derived_data = UpdateContext::builder(project)
        .with_max_concurrent_solves(max_concurrent_solves)
        .with_package_cache(package_cache)
        .with_no_install(options.no_install)
        .with_outdated_environments(outdated)
        .with_lock_file(lock_file)
        .finish()?
        .update()
        .await?;

    // Write the lock-file to disk
    lock_file_derived_data.write_to_disk()?;

    Ok(lock_file_derived_data)
}

pub struct UpdateContextBuilder<'p> {
    /// The project
    project: &'p Project,

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
    max_concurrent_solves: Option<usize>,
}

impl<'p> UpdateContextBuilder<'p> {
    /// The package cache to use during the update process. Prefixes might need
    /// to be instantiated to be able to solve pypi dependencies.
    pub fn with_package_cache(self, package_cache: PackageCache) -> Self {
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

    /// Sets the maximum number of solves that are allowed to run concurrently.
    pub fn with_max_concurrent_solves(self, max_concurrent_solves: usize) -> Self {
        Self {
            max_concurrent_solves: Some(max_concurrent_solves),
            ..self
        }
    }

    /// Construct the context.
    pub fn finish(self) -> miette::Result<UpdateContext<'p>> {
        let project = self.project;
        let package_cache = match self.package_cache {
            Some(package_cache) => package_cache,
            None => PackageCache::new(config::get_cache_dir()?.join("pkgs")),
        };
        let lock_file = self.lock_file;
        let outdated = self.outdated_environments.unwrap_or_else(|| {
            OutdatedEnvironments::from_project_and_lock_file(project, &lock_file)
        });

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
                        locked_env.conda_repodata_records().map(|records| {
                            (
                                env.clone(),
                                records
                                    .into_iter()
                                    .map(|(platform, records)| {
                                        (
                                            platform,
                                            Arc::new(RepoDataRecordsByName::from_iter(records)),
                                        )
                                    })
                                    .collect(),
                            )
                        })
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
                                .pypi_packages()
                                .into_iter()
                                .map(|(platform, records)| {
                                    (platform, Arc::new(PypiRecordsByName::from_iter(records)))
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
                                (
                                    platform,
                                    Arc::new(RepoDataRecordsByName::from_iter(records)),
                                )
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

        let max_concurrent_solves = self
            .max_concurrent_solves
            .unwrap_or_else(default_max_concurrent_solves);

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
            conda_solve_semaphore: Arc::new(Semaphore::new(max_concurrent_solves)),
            pypi_solve_semaphore: Arc::new(Semaphore::new(determine_pypi_solve_permits(project))),

            no_install: self.no_install,
        })
    }
}

impl<'p> UpdateContext<'p> {
    /// Construct a new builder for the update context.
    pub fn builder(project: &'p Project) -> UpdateContextBuilder<'p> {
        UpdateContextBuilder {
            project,
            lock_file: LockFile::default(),
            outdated_environments: None,
            no_install: false,
            package_cache: None,
            max_concurrent_solves: None,
        }
    }

    pub async fn update(mut self) -> miette::Result<LockFileDerivedData<'p>> {
        let project = self.project;
        let current_platform = Platform::current();

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
                    project.repodata_gateway().clone(),
                    platform,
                    self.conda_solve_semaphore.clone(),
                    project.client().clone(),
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

        // Spawn tasks to instantiate prefixes that we need to be able to solve Pypi
        // packages.
        //
        // Solving Pypi packages requires a python interpreter to be present in the
        // prefix, therefore we first need to make sure we have conda packages
        // available, then we can instantiate the prefix with at least the required
        // conda packages (including a python interpreter) and then we can solve the
        // Pypi packages using the installed interpreter.
        //
        // We only need to instantiate the prefix for the current platform.
        for (environment, platforms) in self.outdated_envs.pypi.iter() {
            // Only instantiate a prefix if any of the platforms actually contain pypi
            // dependencies. If there are no pypi-dependencies than solving is also
            // not required and thus a prefix is also not required.
            if !platforms
                .iter()
                .any(|p| !environment.pypi_dependencies(Some(*p)).is_empty())
            {
                continue;
            }

            // If we are not allowed to install, we can't instantiate a prefix.
            if self.no_install {
                miette::bail!("Cannot update pypi dependencies without first installing a conda prefix that includes python.");
            }

            // Check if the group is already being instantiated
            let group = GroupedEnvironment::from(environment.clone());
            if self.instantiated_conda_prefixes.contains_key(&group) {
                continue;
            }

            // Construct a future that will resolve when we have the repodata available for
            // the current platform for this group.
            let records_future = self
                .get_latest_group_repodata_records(&group, environment.best_platform())
                .ok_or_else(|| {
                    make_unsupported_pypi_platform_error(environment, current_platform)
                })?;

            // Spawn a task to instantiate the environment
            let environment_name = environment.name().clone();
            let pypi_env_task =
                spawn_create_prefix_task(group.clone(), self.package_cache.clone(), records_future)
                    .map_err(move |e| {
                        e.context(format!(
                            "failed to instantiate a prefix for '{}'",
                            environment_name
                        ))
                    })
                    .boxed_local();

            pending_futures.push(pypi_env_task);
            let previous_cell = self
                .instantiated_conda_prefixes
                .insert(group, Arc::new(BarrierCell::new()));
            assert!(
                previous_cell.is_none(),
                "cannot update the same group twice"
            )
        }

        // Spawn tasks to update the pypi packages.
        let mut uv_context = None;
        for (environment, platform) in self
            .outdated_envs
            .pypi
            .iter()
            .flat_map(|(env, platforms)| platforms.iter().map(move |p| (env.clone(), *p)))
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

            // Construct a future that will resolve when we have the repodata available
            let repodata_future = self
                .get_latest_group_repodata_records(&group, platform)
                .expect("conda records should be available now or in the future");

            // Construct a future that will resolve when we have the conda prefix available
            let prefix_future = self
                .get_conda_prefix(&group)
                .expect("prefix should be available now or in the future");

            // Get the uv context
            let uv_context = match uv_context.as_ref() {
                None => uv_context
                    .insert(UvResolutionContext::from_project(project)?)
                    .clone(),
                Some(context) => context.clone(),
            };

            // Get environment variables from the activation
            let env_variables = project.get_env_variables(&environment, false).await?;

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
                platform,
                repodata_future,
                prefix_future,
                env_variables,
                self.pypi_solve_semaphore.clone(),
                project.root().to_path_buf(),
                locked_group_records,
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
            .template("{spinner:.cyan} {prefix:20!} [{elapsed_precise}] [{bar:40!.bright.yellow/dim.white}] {pos:>4}/{len:4} {wide_msg:.dim}").unwrap()
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
                TaskResult::CondaPrefixUpdated(group_name, prefix, python_status, duration) => {
                    let group = GroupedEnvironment::from_name(project, &group_name)
                        .expect("grouped environment should exist");

                    self.instantiated_conda_prefixes
                        .get_mut(&group)
                        .expect("the entry for this environment should exists")
                        .set((prefix, *python_status))
                        .expect("prefix should not be instantiated twice");

                    tracing::info!(
                        "updated conda packages in the '{}' prefix in {}",
                        group.name().fancy_display(),
                        humantime::format_duration(duration)
                    );
                }
                TaskResult::PypiGroupSolved(group_name, platform, records, duration) => {
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

            builder.set_channels(
                &environment_name,
                grouped_env
                    .channels()
                    .into_iter()
                    .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string())),
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
            project,
            lock_file,
            updated_conda_prefixes: self.take_instantiated_conda_prefixes(),
            package_cache: self.package_cache,
            updated_pypi_prefixes: HashMap::default(),
            uv_context,
        })
    }
}

/// Constructs an error that indicates that the current platform cannot solve
/// pypi dependencies because there is no python interpreter available for the
/// current platform.
fn make_unsupported_pypi_platform_error(
    environment: &Environment<'_>,
    current_platform: Platform,
) -> miette::Report {
    let grouped_environment = GroupedEnvironment::from(environment.clone());

    // Construct a diagnostic that explains that the current platform is not
    // supported.
    let mut diag = MietteDiagnostic::new(format!("Unable to solve pypi dependencies for the {} {} because no compatible python interpreter can be installed for the current platform", grouped_environment.name().fancy_display(), match &grouped_environment {
        GroupedEnvironment::Group(_) => "solve group",
        GroupedEnvironment::Environment(_) => "environment"
    }));

    let mut labels = Vec::new();

    // Add a reference to the set of platforms that are supported by the project.
    let project_platforms = &environment.project().manifest.parsed.project.platforms;
    if let Some(span) = project_platforms.span.clone() {
        labels.push(LabeledSpan::at(
            span,
            format!("even though the projects does include support for '{current_platform}'"),
        ));
    }

    // Find all the features that are excluding the current platform.
    let features_without_platform = grouped_environment.features().filter_map(|feature| {
        let platforms = feature.platforms.as_ref()?;
        if !platforms.value.contains(&current_platform) {
            Some((feature, platforms))
        } else {
            None
        }
    });

    for (feature, platforms) in features_without_platform {
        let Some(span) = platforms.span.as_ref() else {
            continue;
        };

        labels.push(LabeledSpan::at(
            span.clone(),
            format!(
                "feature '{}' does not support '{current_platform}'",
                feature.name
            ),
        ));
    }

    diag.labels = Some(labels);
    diag.help = Some("Try converting your [pypi-dependencies] to conda [dependencies]".to_string());

    miette::Report::new(diag).with_source_code(environment.project().manifest.contents.clone())
}

/// Represents data that is sent back from a task. This is used to communicate
/// the result of a task back to the main task which will forward the
/// information to other tasks waiting for results.
enum TaskResult {
    /// The conda dependencies for a grouped environment have been solved.
    CondaGroupSolved(
        GroupedEnvironmentName,
        Platform,
        RepoDataRecordsByName,
        Duration,
    ),

    /// A prefix was updated with the latest conda packages
    CondaPrefixUpdated(GroupedEnvironmentName, Prefix, Box<PythonStatus>, Duration),

    /// The pypi dependencies for a grouped environment have been solved.
    PypiGroupSolved(
        GroupedEnvironmentName,
        Platform,
        PypiRecordsByName,
        Duration,
    ),

    /// The records for a specific environment have been extracted from a
    /// grouped solve.
    ExtractedRecordsSubset(
        EnvironmentName,
        Platform,
        Arc<RepoDataRecordsByName>,
        Arc<PypiRecordsByName>,
    ),
}

/// A task that solves the conda dependencies for a given environment.
async fn spawn_solve_conda_environment_task(
    group: GroupedEnvironment<'_>,
    existing_repodata_records: Arc<RepoDataRecordsByName>,
    repodata_gateway: Gateway,
    platform: Platform,
    concurrency_semaphore: Arc<Semaphore>,
    client: reqwest::Client,
) -> miette::Result<TaskResult> {
    // Get the dependencies for this platform
    let dependencies = group.dependencies(None, Some(platform));

    // Get the virtual packages for this platform
    let virtual_packages = group.virtual_packages(platform);

    // Get the environment name
    let group_name = group.name();

    // The list of channels and platforms we need for this task
    let channels = group.channels().into_iter().cloned().collect_vec();

    // Whether there are pypi dependencies, and we should fetch purls.
    let has_pypi_dependencies = group.has_pypi_dependencies();

    // Whether we should use custom mapping location
    let pypi_name_mapping_location = group.project().pypi_name_mapping_source().clone();

    tokio::spawn(
        async move {
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

            let start = Instant::now();

            // Convert the dependencies into match specs
            let match_specs = dependencies
                .iter_specs()
                .map(|(name, constraint)| {
                    MatchSpec::from_nameless(constraint.clone(), Some(name.clone()))
                })
                .collect_vec();

            // Extract the repo data records needed to solve the environment.
            pb.set_message("loading repodata");
            let fetch_repodata_start = Instant::now();
            let available_packages = repodata_gateway
                .query(
                    channels,
                    [platform, Platform::NoArch],
                    dependencies.clone().into_match_specs(),
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
            let mut records = lock_file::resolve_conda(
                match_specs,
                virtual_packages,
                existing_repodata_records.records.clone(),
                available_packages,
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
                pb.set_message("extracting pypi packages");
                pypi_mapping::amend_pypi_purls(
                    client,
                    &pypi_name_mapping_location,
                    &mut records,
                    Some(pb.purl_amend_reporter()),
                )
                .await?;
            }

            // Turn the records into a map by name
            let records_by_name = RepoDataRecordsByName::from(records);

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
    grouped_repodata_records: impl Future<Output = Arc<RepoDataRecordsByName>>,
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
    let conda_package_identifiers = grouped_repodata_records.by_pypi_name();

    #[derive(Clone, Eq, PartialEq, Hash)]
    enum PackageName {
        Conda(rattler_conda_types::PackageName),
        Pypi((uv_normalize::PackageName, Option<ExtraName>)),
    }

    enum PackageRecord<'a> {
        Conda(&'a RepoDataRecord),
        Pypi((&'a PypiRecord, Option<ExtraName>)),
    }

    // Determine the conda packages we need.
    let conda_package_names = environment
        .dependencies(None, Some(platform))
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
        for req in reqs {
            for extra in req.extras().iter() {
                pypi_package_names.insert(PackageName::Pypi((name.clone(), Some(extra.clone()))));
            }
        }
        pypi_package_names.insert(PackageName::Pypi((name, None)));
    }

    // Compute the Pypi marker environment. Only do this if we have pypi
    // dependencies.
    let marker_environment = if has_pypi_dependencies {
        grouped_repodata_records
            .records
            .iter()
            .find(|r| is_python_record(r))
            .and_then(|record| determine_marker_environment(platform, &record.package_record).ok())
    } else {
        None
    };

    // Construct a queue of packages that we need to check.
    let mut queue = itertools::chain(conda_package_names, pypi_package_names).collect::<Vec<_>>();
    let mut queued_names = queue.iter().cloned().collect::<HashSet<_>>();

    let mut conda_records = Vec::new();
    let mut pypi_records = HashMap::new();
    while let Some(package) = queue.pop() {
        let record = match package {
            PackageName::Conda(name) => grouped_repodata_records
                .by_name(&name)
                .map(PackageRecord::Conda),
            PackageName::Pypi((name, extra)) => {
                if let Some(found_record) = grouped_pypi_records.by_name(&name) {
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
                for dependency in record.package_record.depends.iter() {
                    let dependency_name =
                        PackageName::Conda(rattler_conda_types::PackageName::new_unchecked(
                            dependency.split_once(' ').unwrap_or((&dependency, "")).0,
                        ));
                    if queued_names.insert(dependency_name.clone()) {
                        queue.push(dependency_name);
                    }
                }

                // Store the record itself as part of the subset
                conda_records.push(record);
            }
            PackageRecord::Pypi((record, extra)) => {
                // Evaluate all dependencies
                let extras = extra.map(|extra| vec![extra]).unwrap_or_default();
                for req in record.0.requires_dist.iter() {
                    // Evaluate the marker environment with the given extras
                    if let Some(marker_env) = &marker_environment {
                        if !req.evaluate_markers(marker_env, &extras) {
                            continue;
                        }
                    }

                    // Add the package to the queue
                    for extra in req.extras.iter() {
                        if queued_names
                            .insert(PackageName::Pypi((req.name.clone(), Some(extra.clone()))))
                        {
                            queue.push(PackageName::Pypi((req.name.clone(), Some(extra.clone()))));
                        }
                    }

                    // Also add the dependency without any extras
                    queue.push(PackageName::Pypi((req.name.clone(), None)));
                }

                // Insert the record if it is not already present
                pypi_records.entry(record.0.name.clone()).or_insert(record);
            }
        }
    }

    Ok(TaskResult::ExtractedRecordsSubset(
        environment.name().clone(),
        platform,
        Arc::new(RepoDataRecordsByName::from_iter(
            conda_records.into_iter().cloned(),
        )),
        Arc::new(PypiRecordsByName::from_iter(
            pypi_records.into_values().cloned(),
        )),
    ))
}

/// A task that solves the pypi dependencies for a given environment.
#[allow(clippy::too_many_arguments)]
async fn spawn_solve_pypi_task(
    resolution_context: UvResolutionContext,
    environment: GroupedEnvironment<'_>,
    platform: Platform,
    repodata_records: impl Future<Output = Arc<RepoDataRecordsByName>>,
    prefix: impl Future<Output = (Prefix, PythonStatus)>,
    env_variables: &HashMap<String, String>,
    semaphore: Arc<Semaphore>,
    project_root: PathBuf,
    locked_pypi_packages: Arc<PypiRecordsByName>,
) -> miette::Result<TaskResult> {
    // Get the Pypi dependencies for this environment
    let dependencies = environment.pypi_dependencies(Some(platform));
    if dependencies.is_empty() {
        return Ok(TaskResult::PypiGroupSolved(
            environment.name().clone(),
            platform,
            PypiRecordsByName::default(),
            Duration::from_millis(0),
        ));
    }

    // Get the system requirements for this environment
    let system_requirements = environment.system_requirements();

    // Wait until the conda records and prefix are available.
    let (repodata_records, (prefix, python_status), _guard) =
        tokio::join!(repodata_records, prefix, semaphore.acquire_owned());

    let environment_name = environment.name().clone();

    let pypi_name_mapping_location = environment.project().pypi_name_mapping_source();

    let mut conda_records = repodata_records.records.clone();
    let locked_pypi_records = locked_pypi_packages.records.clone();

    pypi_mapping::amend_pypi_purls(
        environment.project().client().clone(),
        pypi_name_mapping_location,
        &mut conda_records,
        None,
    )
    .await?;

    let pypi_options = environment.pypi_options();
    // let (pypi_packages, duration) = tokio::spawn(
    let (pypi_packages, duration) = async move {
        let pb = SolveProgressBar::new(
            global_multi_progress().add(ProgressBar::hidden()),
            platform,
            environment_name.clone(),
        );
        pb.start();

        let python_path = python_status
            .location()
            .map(|path| prefix.root().join(path))
            .ok_or_else(|| {
                miette::miette!(
                    help = "Use `pixi add python` to install the latest python interpreter.",
                    "missing python interpreter from environment"
                )
            })?;

        let start = Instant::now();

        let records = lock_file::resolve_pypi(
            resolution_context,
            &pypi_options,
            dependencies
                .into_iter()
                .map(|(name, requirement)| (name.as_normalized().clone(), requirement))
                .collect(),
            system_requirements,
            &conda_records,
            &locked_pypi_records,
            platform,
            &pb.pb,
            &python_path,
            env_variables,
            &project_root,
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

        Ok::<(_, _), miette::Report>((PypiRecordsByName::from_iter(records), end - start))
    }
    .instrument(tracing::info_span!(
        "resolve_pypi",
        group = %environment.name().as_str(),
        platform = %platform
    ))
    .await?;

    Ok(TaskResult::PypiGroupSolved(
        environment.name().clone(),
        platform,
        pypi_packages,
        duration,
    ))
}

/// Updates the prefix for the given environment.
///
/// This function will wait until the conda records for the prefix are
/// available.
async fn spawn_create_prefix_task(
    group: GroupedEnvironment<'_>,
    package_cache: PackageCache,
    conda_records: impl Future<Output = Arc<RepoDataRecordsByName>>,
) -> miette::Result<TaskResult> {
    let group_name = group.name().clone();
    let prefix = group.prefix();
    let client = group.project().authenticated_client().clone();

    // Spawn a task to determine the currently installed packages.
    let installed_packages_future = tokio::spawn({
        let prefix = prefix.clone();
        async move { prefix.find_installed_packages(None).await }
    })
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    });

    // Wait until the conda records are available and until the installed packages
    // for this prefix are available.
    let (conda_records, installed_packages) =
        tokio::try_join!(conda_records.map(Ok), installed_packages_future)?;

    // Spawn a background task to update the prefix
    let (python_status, duration) = tokio::spawn({
        let prefix = prefix.clone();
        let group_name = group_name.clone();
        async move {
            let start = Instant::now();
            let has_existing_packages = !installed_packages.is_empty();
            let python_status = environment::update_prefix_conda(
                &prefix,
                package_cache,
                client,
                installed_packages,
                conda_records.records.clone(),
                Platform::current(),
                &format!(
                    "{} python environment to solve pypi packages for '{}'",
                    if has_existing_packages {
                        "updating"
                    } else {
                        "creating"
                    },
                    group_name.fancy_display()
                ),
                "  ",
            )
            .await?;
            let end = Instant::now();
            Ok((python_status, end - start))
        }
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })?;

    Ok(TaskResult::CondaPrefixUpdated(
        group_name,
        prefix,
        Box::new(python_status),
        duration,
    ))
}

/// A helper struct that manages a progress-bar for solving an environment.
#[derive(Clone)]
pub(crate) struct SolveProgressBar {
    pub pb: ProgressBar,
}

impl SolveProgressBar {
    pub fn new(
        pb: ProgressBar,
        platform: Platform,
        environment_name: GroupedEnvironmentName,
    ) -> Self {
        let name_and_platform = format!(
            "{}:{}",
            environment_name.fancy_display(),
            consts::PLATFORM_STYLE.apply_to(platform)
        );

        pb.set_style(indicatif::ProgressStyle::with_template("    {prefix:20!} ..").unwrap());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_prefix(name_and_platform);
        Self { pb }
    }

    pub fn start(&self) {
        self.pb.reset_elapsed();
        self.reset_style()
    }

    pub fn set_message(&self, msg: impl Into<Cow<'static, str>>) {
        self.pb.set_message(msg);
    }

    pub fn inc(&self, n: u64) {
        self.pb.inc(n);
    }

    pub fn set_position(&self, n: u64) {
        self.pb.set_position(n)
    }

    pub fn set_update_style(&self, total: usize) {
        self.pb.set_length(total as u64);
        self.pb.set_position(0);
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {pos:>4}/{len:4} {msg:.dim}")
                .unwrap()
                .progress_chars("━━╾─"),
        );
    }

    pub fn set_bytes_update_style(&self, total: usize) {
        self.pb.set_length(total as u64);
        self.pb.set_position(0);
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] [{bar:20!.bright.yellow/dim.white}] {bytes:>8} @ {smoothed_bytes_per_sec:8} {msg:.dim}")
                .unwrap()
                .progress_chars("━━╾─")
                .with_key(
                    "smoothed_bytes_per_sec",
                    |s: &ProgressState, w: &mut dyn Write| match (s.pos(), s.elapsed().as_millis()) {
                        (pos, elapsed_ms) if elapsed_ms > 0 => {
                            write!(w, "{}/s", HumanBytes((pos as f64 * 1000_f64 / elapsed_ms as f64) as u64)).unwrap()
                        }
                        _ => write!(w, "-").unwrap(),
                    },
                )
        );
    }

    pub fn reset_style(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner:.dim} {prefix:20!} [{elapsed_precise}] {msg:.dim}",
            )
            .unwrap(),
        );
    }

    pub fn finish(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {} {{prefix:20!}} [{{elapsed_precise}}]",
                console::style(console::Emoji("✔", "↳")).green(),
            ))
            .unwrap(),
        );
        self.pb.finish_and_clear();
    }

    fn purl_amend_reporter(self: &Arc<Self>) -> Arc<dyn Reporter> {
        Arc::new(PurlAmendReporter {
            pb: self.clone(),
            style_set: AtomicBool::new(false),
        })
    }
}

struct PurlAmendReporter {
    pb: Arc<SolveProgressBar>,
    style_set: AtomicBool,
}

impl pypi_mapping::Reporter for PurlAmendReporter {
    fn download_started(&self, _package: &RepoDataRecord, total: usize) {
        if !self.style_set.swap(true, Ordering::Relaxed) {
            self.pb.set_update_style(total);
        }
    }

    fn download_finished(&self, _package: &RepoDataRecord, _total: usize) {
        self.pb.inc(1);
    }

    fn download_failed(&self, package: &RepoDataRecord, total: usize) {
        self.download_finished(package, total);
    }
}

struct GatewayProgressReporter {
    inner: Mutex<InnerProgressState>,
}

impl GatewayProgressReporter {
    pub fn new(pb: Arc<SolveProgressBar>) -> Self {
        Self {
            inner: Mutex::new(InnerProgressState {
                pb,
                downloads: VecDeque::new(),

                bytes_downloaded: 0,
                total_bytes: 0,
                total_pending_downloads: 0,

                jlap: VecDeque::default(),
                total_pending_jlap: 0,
            }),
        }
    }
}

struct InnerProgressState {
    pb: Arc<SolveProgressBar>,

    downloads: VecDeque<DownloadState>,

    bytes_downloaded: usize,
    total_bytes: usize,
    total_pending_downloads: usize,

    jlap: VecDeque<JLAPState>,
    total_pending_jlap: usize,
}

impl InnerProgressState {
    fn update_progress(&self) {
        if self.total_pending_downloads > 0 {
            self.pb.set_bytes_update_style(self.total_bytes);
            self.pb.set_position(self.bytes_downloaded as u64);
            self.pb.set_message("downloading repodata");
        } else if self.total_pending_jlap > 0 {
            self.pb.reset_style();
            self.pb.set_message("applying JLAP patches");
        } else {
            self.pb.reset_style();
            self.pb.set_message("parsing repodata");
        }
    }
}

struct DownloadState {
    _started_at: Instant,
    bytes_downloaded: usize,
    total_size: usize,
    _finished_at: Option<Instant>,
}

struct JLAPState {
    _started_at: Instant,
    _finished_at: Option<Instant>,
}

impl rattler_repodata_gateway::Reporter for GatewayProgressReporter {
    fn on_download_start(&self, _url: &Url) -> usize {
        let mut inner = self.inner.lock();
        let download_idx = inner.downloads.len();
        inner.downloads.push_back(DownloadState {
            _started_at: Instant::now(),
            bytes_downloaded: 0,
            total_size: 0,
            _finished_at: None,
        });
        inner.total_pending_downloads += 1;
        inner.update_progress();
        download_idx
    }

    fn on_download_progress(
        &self,
        _url: &Url,
        index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        let mut inner = self.inner.lock();

        let download = inner
            .downloads
            .get_mut(index)
            .expect("download index should exist");

        let prev_bytes_downloaded = download.bytes_downloaded;
        let prev_total_size = download.total_size;
        download.bytes_downloaded = bytes_downloaded;
        download.total_size = total_bytes.unwrap_or(0);

        inner.bytes_downloaded = inner.bytes_downloaded + bytes_downloaded - prev_bytes_downloaded;
        inner.total_bytes = inner.total_bytes + total_bytes.unwrap_or(0) - prev_total_size;

        inner.update_progress();
    }

    fn on_download_complete(&self, _url: &Url, _index: usize) {
        let mut inner = self.inner.lock();
        let download = inner
            .downloads
            .get_mut(_index)
            .expect("download index should exist");
        download._finished_at = Some(Instant::now());

        inner.total_pending_downloads -= 1;

        inner.update_progress();
    }

    fn on_jlap_start(&self) -> usize {
        let mut inner = self.inner.lock();

        let index = inner.jlap.len();
        inner.jlap.push_back(JLAPState {
            _started_at: Instant::now(),
            _finished_at: None,
        });
        inner.total_pending_jlap += 1;

        inner.update_progress();

        index
    }

    fn on_jlap_completed(&self, index: usize) {
        let mut inner = self.inner.lock();
        let jlap = inner.jlap.get_mut(index).expect("jlap index should exist");
        jlap._finished_at = Some(Instant::now());
        inner.total_pending_jlap -= 1;

        inner.update_progress();
    }
}
