use miette::{Context, IntoDiagnostic};

use crate::lock_file::{resolve_pypi, LockedCondaPackages, LockedPypiPackages};
use crate::{
    config, consts, install, install_pypi, lock_file,
    lock_file::{
        load_lock_file, verify_environment_satisfiability, verify_platform_satisfiability,
        PlatformUnsat,
    },
    prefix::Prefix,
    progress::{self, global_multi_progress},
    project::{
        manifest::{EnvironmentName, SystemRequirements},
        virtual_packages::verify_current_platform_has_required_virtual_packages,
        Environment,
    },
    repodata::fetch_sparse_repodata_targets,
    utils::BarrierCell,
    Project,
};
use futures::future::Either;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt, TryFutureExt};
use indexmap::{IndexMap, IndexSet};
use indicatif::ProgressBar;
use itertools::Itertools;
use rattler::install::{PythonInfo, Transaction};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_lock::{LockFile, PypiPackageData, PypiPackageEnvironmentData};
use rattler_repodata_gateway::sparse::SparseRepoData;
use reqwest_middleware::ClientWithMiddleware;
use rip::{index::PackageDb, resolve::SDistResolution};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    convert::identity,
    future::{ready, Future},
    io::ErrorKind,
    path::Path,
    sync::Arc,
    time::Duration,
};

/// Verify the location of the prefix folder is not changed so the applied prefix path is still valid.
/// Errors when there is a file system error or the path does not align with the defined prefix.
/// Returns false when the file is not present.
pub fn verify_prefix_location_unchanged(prefix_file: &Path) -> miette::Result<()> {
    match std::fs::read_to_string(prefix_file) {
        // Not found is fine as it can be new or backwards compatible.
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        // Scream the error if we dont know it.
        Err(e) => Err(e).into_diagnostic(),
        // Check if the path in the file aligns with the current path.
        Ok(p) if prefix_file.starts_with(&p) => Ok(()),
        Ok(p) => Err(miette::miette!(
            "the project location seems to be change from `{}` to `{}`, this is not allowed.\
            \nPlease remove the `{}` folder and run again",
            p,
            prefix_file
                .parent()
                .expect("prefix_file should always be a file")
                .display(),
            consts::PIXI_DIR
        )),
    }
}

/// Create the prefix location file.
/// Give it the file path of the required location to place it.
fn create_prefix_location_file(prefix_file: &Path) -> miette::Result<()> {
    let parent = prefix_file
        .parent()
        .ok_or_else(|| miette::miette!("cannot find parent of '{}'", prefix_file.display()))?;

    if parent.exists() {
        let contents = parent.to_str().ok_or_else(|| {
            miette::miette!("failed to convert path to str: '{}'", parent.display())
        })?;
        std::fs::write(prefix_file, contents).into_diagnostic()?;
    }
    Ok(())
}

/// Runs the following checks to make sure the project is in a sane state:
///     1. It verifies that the prefix location is unchanged.
///     2. It verifies that the system requirements are met.
///     3. It verifies the absence of the `env` folder.
pub fn sanity_check_project(project: &Project) -> miette::Result<()> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(
        project
            .default_environment()
            .dir()
            .join(consts::PREFIX_FILE_NAME)
            .as_path(),
    )?;

    // Make sure the system requirements are met
    verify_current_platform_has_required_virtual_packages(&project.default_environment())?;

    // TODO: remove on a 1.0 release
    // Check for old `env` folder as we moved to `envs` in 0.13.0
    let old_pixi_env_dir = project.pixi_dir().join("env");
    if old_pixi_env_dir.exists() {
        tracing::warn!(
            "The `{}` folder is deprecated, please remove it as we now use the `{}` folder",
            old_pixi_env_dir.display(),
            consts::ENVIRONMENTS_DIR
        );
    }

    Ok(())
}

/// Specifies how the lock-file should be updated.
#[derive(Debug, Default, PartialEq, Eq, Copy, Clone)]
pub enum LockFileUsage {
    /// Update the lock-file if it is out of date.
    #[default]
    Update,
    /// Don't update the lock-file, but do check if it is out of date
    Locked,
    /// Don't update the lock-file and don't check if it is out of date
    Frozen,
}

impl LockFileUsage {
    /// Returns true if the lock-file should be updated if it is out of date.
    pub fn allows_lock_file_updates(self) -> bool {
        match self {
            LockFileUsage::Update => true,
            LockFileUsage::Locked | LockFileUsage::Frozen => false,
        }
    }

    /// Returns true if the lock-file should be checked if it is out of date.
    pub fn should_check_if_out_of_date(self) -> bool {
        match self {
            LockFileUsage::Update | LockFileUsage::Locked => true,
            LockFileUsage::Frozen => false,
        }
    }
}

/// Returns the prefix associated with the given environment. If the prefix doesn't exist or is not
/// up-to-date it is updated.
///
/// The `sparse_repo_data` is used when the lock-file is update. We pass it into this function to
/// make sure the data is not loaded twice since the repodata takes up a lot of memory and takes a
/// while to load. If `sparse_repo_data` is `None` it will be downloaded. If the lock-file is not
/// updated, the `sparse_repo_data` is ignored.
pub async fn get_up_to_date_prefix(
    environment: &Environment<'_>,
    lock_file_usage: LockFileUsage,
    mut no_install: bool,
    existing_repo_data: IndexMap<(Channel, Platform), SparseRepoData>,
) -> miette::Result<Prefix> {
    let current_platform = Platform::current();
    let project = environment.project();

    // Do not install if the platform is not supported
    if !no_install && !environment.platforms().contains(&current_platform) {
        tracing::warn!("Not installing dependency on current platform: ({current_platform}) as it is not part of this project's supported platforms.");
        no_install = true;
    }

    // Make sure the project is in a sane state
    sanity_check_project(project)?;

    // Ensure that the lock-file is up-to-date
    let mut lock_file = project
        .up_to_date_lock_file(UpdateLockFileOptions {
            existing_repo_data,
            lock_file_usage,
            no_install,
        })
        .await?;

    // Get the locked environment from the lock-file.
    if no_install {
        Ok(Prefix::new(environment.dir()))
    } else {
        lock_file.prefix(environment).await
    }
}

/// Options to pass to [`Project::up_to_date_lock_file`].
#[derive(Default)]
pub struct UpdateLockFileOptions {
    /// Defines what to do if the lock-file is out of date
    pub lock_file_usage: LockFileUsage,

    /// Don't install anything to disk.
    pub no_install: bool,

    /// Existing repodata that can be used to avoid downloading it again.
    pub existing_repo_data: IndexMap<(Channel, Platform), SparseRepoData>,
}

impl Project {
    /// Ensures that the lock-file is up-to-date with the project information.
    ///
    /// Returns the lock-file and any potential derived data that was computed as part of this
    /// operation.
    pub async fn up_to_date_lock_file(
        &self,
        options: UpdateLockFileOptions,
    ) -> miette::Result<LockFileDerivedData<'_>> {
        ensure_up_to_date_lock_file(
            self,
            options.existing_repo_data,
            options.lock_file_usage,
            options.no_install,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
// TODO: refactor args into struct
pub async fn update_prefix_pypi(
    name: &str,
    prefix: &Prefix,
    platform: Platform,
    package_db: Arc<PackageDb>,
    conda_records: &[RepoDataRecord],
    pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
    status: &PythonStatus,
    system_requirements: &SystemRequirements,
    sdist_resolution: SDistResolution,
) -> miette::Result<()> {
    // Remove python packages from a previous python distribution if the python version changed.
    install_pypi::remove_old_python_distributions(prefix, platform, status)?;

    // Install and/or remove python packages
    progress::await_in_progress(format!("updating pypi package in '{}'", name), |_| {
        install_pypi::update_python_distributions(
            package_db,
            prefix,
            conda_records,
            pypi_records,
            platform,
            status,
            system_requirements,
            sdist_resolution,
        )
    })
    .await
}

#[derive(Clone)]
pub enum PythonStatus {
    /// The python interpreter changed from `old` to `new`.
    Changed { old: PythonInfo, new: PythonInfo },

    /// The python interpreter remained the same.
    Unchanged(PythonInfo),

    /// The python interpreter was removed from the environment
    Removed { old: PythonInfo },

    /// The python interpreter was added to the environment
    Added { new: PythonInfo },

    /// There is no python interpreter in the environment.
    DoesNotExist,
}

impl PythonStatus {
    /// Determine the [`PythonStatus`] from a [`Transaction`].
    pub fn from_transaction(transaction: &Transaction<PrefixRecord, RepoDataRecord>) -> Self {
        match (
            transaction.current_python_info.as_ref(),
            transaction.python_info.as_ref(),
        ) {
            (Some(old), Some(new)) if old.short_version != new.short_version => {
                PythonStatus::Changed {
                    old: old.clone(),
                    new: new.clone(),
                }
            }
            (Some(_), Some(new)) => PythonStatus::Unchanged(new.clone()),
            (None, Some(new)) => PythonStatus::Added { new: new.clone() },
            (Some(old), None) => PythonStatus::Removed { old: old.clone() },
            (None, None) => PythonStatus::DoesNotExist,
        }
    }

    /// Returns the info of the current situation (e.g. after the transaction completed).
    pub fn current_info(&self) -> Option<&PythonInfo> {
        match self {
            PythonStatus::Changed { new, .. }
            | PythonStatus::Unchanged(new)
            | PythonStatus::Added { new } => Some(new),
            PythonStatus::Removed { .. } | PythonStatus::DoesNotExist => None,
        }
    }

    /// Returns the location of the python interpreter relative to the root of the prefix.
    pub fn location(&self) -> Option<&Path> {
        Some(&self.current_info()?.path)
    }
}

/// Updates the environment to contain the packages from the specified lock-file
pub async fn update_prefix_conda(
    name: &str,
    prefix: &Prefix,
    package_cache: Arc<PackageCache>,
    authenticated_client: ClientWithMiddleware,
    installed_packages: Vec<PrefixRecord>,
    repodata_records: &[RepoDataRecord],
    platform: Platform,
) -> miette::Result<PythonStatus> {
    // Construct a transaction to bring the environment up to date with the lock-file content
    let transaction = Transaction::from_current_and_desired(
        installed_packages.clone(),
        // TODO(baszalmstra): Can we avoid cloning here?
        repodata_records.to_owned(),
        platform,
    )
    .into_diagnostic()?;

    // Execute the transaction if there is work to do
    if !transaction.operations.is_empty() {
        // Execute the operations that are returned by the solver.
        progress::await_in_progress(format!("updating packages in '{}'", name), |pb| async {
            install::execute_transaction(
                package_cache,
                &transaction,
                &installed_packages,
                prefix.root().to_path_buf(),
                authenticated_client,
                pb,
            )
            .await
        })
        .await?;
    }

    // Mark the location of the prefix
    create_prefix_location_file(&prefix.root().join(consts::PREFIX_FILE_NAME))?;

    // Determine if the python version changed.
    Ok(PythonStatus::from_transaction(&transaction))
}

/// A struct that holds the lock-file and any potential derived data that was computed when calling
/// `ensure_up_to_date_lock_file`.
pub struct LockFileDerivedData<'p> {
    /// The lock-file
    pub lock_file: LockFile,

    /// The package cache
    pub package_cache: Arc<PackageCache>,

    /// Repodata that was fetched
    pub repo_data: IndexMap<(Channel, Platform), SparseRepoData>,

    /// A list of prefixes that are up-to-date with the latest conda packages.
    pub updated_conda_prefixes: HashMap<Environment<'p>, (Prefix, PythonStatus)>,

    /// A list of prefixes that have been updated while resolving all dependencies.
    pub updated_pypi_prefixes: HashMap<Environment<'p>, Prefix>,
}

impl<'p> LockFileDerivedData<'p> {
    /// Returns the up-to-date prefix for the given environment.
    pub async fn prefix(&mut self, environment: &Environment<'p>) -> miette::Result<Prefix> {
        if let Some(prefix) = self.updated_pypi_prefixes.get(environment) {
            return Ok(prefix.clone());
        }

        // Get the prefix with the conda packages installed.
        let platform = Platform::current();
        let package_db = environment.project().pypi_package_db()?;
        let (prefix, python_status) = self.conda_prefix(environment).await?;
        let repodata_records = self
            .repodata_records(environment, platform)
            .unwrap_or_default();
        let pypi_records = self.pypi_records(environment, platform).unwrap_or_default();

        // Update the prefix with Pypi records
        update_prefix_pypi(
            environment.name().as_str(),
            &prefix,
            platform,
            package_db,
            &repodata_records,
            &pypi_records,
            &python_status,
            &environment.system_requirements(),
            SDistResolution::default(),
        )
        .await?;

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
        let platform = Platform::current();

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
        let python_status = update_prefix_conda(
            environment.name().as_str(),
            &prefix,
            self.package_cache.clone(),
            environment.project().authenticated_client().clone(),
            installed_packages,
            &records,
            platform,
        )
        .await?;

        // Store that we updated the environment, so we won't have to do it again.
        self.updated_conda_prefixes
            .insert(environment.clone(), (prefix.clone(), python_status.clone()));

        Ok((prefix, python_status))
    }
}

/// A struct that defines which targets are out of date.
struct OutdatedEnvironments<'p> {
    conda: HashMap<Environment<'p>, HashSet<Platform>>,
    pypi: HashMap<Environment<'p>, HashSet<Platform>>,
}

impl<'p> OutdatedEnvironments<'p> {
    pub fn from_project_and_lock_file(project: &'p Project, lock_file: &LockFile) -> Self {
        let mut outdated_conda: HashMap<_, HashSet<_>> = HashMap::new();
        let mut outdated_pypi: HashMap<_, HashSet<_>> = HashMap::new();

        for environment in project.environments() {
            let platforms = environment.platforms();

            // Get the locked environment from the environment
            let Some(locked_environment) = lock_file.environment(environment.name().as_str())
            else {
                tracing::info!(
                    "environment '{0}' is out of date because it does not exist in the lock-file.",
                    environment.name().fancy_display()
                );

                outdated_conda
                    .entry(environment.clone())
                    .or_default()
                    .extend(platforms);

                continue;
            };

            // The locked environment exists, but does it match our project environment?
            if let Err(unsat) = verify_environment_satisfiability(&environment, &locked_environment)
            {
                tracing::info!(
                    "environment '{0}' is out of date because {unsat}",
                    environment.name().fancy_display()
                );

                outdated_conda
                    .entry(environment.clone())
                    .or_default()
                    .extend(platforms);

                continue;
            }

            // Verify each individual platform
            for platform in platforms {
                match verify_platform_satisfiability(&environment, &locked_environment, platform) {
                    Ok(_) => {}
                    Err(unsat @ PlatformUnsat::UnsatisfiableRequirement(_, _)) => {
                        tracing::info!(
                        "the pypi dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                        environment.name().fancy_display()
                    );

                        outdated_pypi
                            .entry(environment.clone())
                            .or_default()
                            .insert(platform);
                    }
                    Err(unsat) => {
                        tracing::info!(
                        "the dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                        environment.name().fancy_display()
                    );

                        outdated_conda
                            .entry(environment.clone())
                            .or_default()
                            .insert(platform);
                    }
                }
            }
        }

        // For all targets where conda is out of date, the pypi packages are also out of date.
        for (environment, platforms) in outdated_conda.iter() {
            outdated_pypi
                .entry(environment.clone())
                .or_default()
                .extend(platforms.iter().copied());
        }

        Self {
            conda: outdated_conda,
            pypi: outdated_pypi,
        }
    }

    /// Returns true if the lock-file is up-to-date with the project.
    pub fn is_empty(&self) -> bool {
        self.conda.is_empty() && self.pypi.is_empty()
    }
}

type PerEnvironment<'p, T> = HashMap<Environment<'p>, T>;
type PerEnvironmentAndPlatform<'p, T> = HashMap<Environment<'p>, HashMap<Platform, T>>;

#[derive(Default)]
struct UpdateContext<'p> {
    /// Repodata that is available to the solve tasks.
    repo_data: Arc<IndexMap<(Channel, Platform), SparseRepoData>>,

    /// Repodata records from the lock-file. This contains the records that actually exist in the
    /// lock-file. If the lock-file is missing or partially missing then the data also won't exist
    /// in this field.
    locked_repodata_records: PerEnvironmentAndPlatform<'p, Arc<LockedCondaPackages>>,

    /// Repodata records from the lock-file. This contains the records that actually exist in the
    /// lock-file. If the lock-file is missing or partially missing then the data also won't exist
    /// in this field.
    locked_pypi_records: PerEnvironmentAndPlatform<'p, Arc<LockedPypiPackages>>,

    /// Keeps track of all pending conda targets that are being solved. The mapping contains a
    /// [`BarrierCell`] that will eventually contain the solved records computed by another task.
    /// This allows tasks to wait for the records to be solved before proceeding.
    solved_repodata_records:
        PerEnvironmentAndPlatform<'p, Arc<BarrierCell<Arc<LockedCondaPackages>>>>,

    /// Keeps track of all pending prefix updates. This only tracks the conda updates to a prefix,
    /// not whether the pypi packages have also been updated.
    instantiated_conda_prefixes: PerEnvironment<'p, Arc<BarrierCell<(Prefix, PythonStatus)>>>,

    /// Keeps track of all pending conda targets that are being solved. The mapping contains a
    /// [`BarrierCell`] that will eventually contain the solved records computed by another task.
    /// This allows tasks to wait for the records to be solved before proceeding.
    solved_pypi_records: PerEnvironmentAndPlatform<'p, Arc<BarrierCell<Arc<LockedPypiPackages>>>>,
}

impl<'p> UpdateContext<'p> {
    /// Returns a future that will resolve to the solved repodata records for the given environment
    /// or `None` if the records do not exist and are also not in the process of being updated.
    pub fn get_latest_repodata_records(
        &self,
        environment: &Environment<'_>,
        platform: Platform,
    ) -> Option<impl Future<Output = Arc<Vec<RepoDataRecord>>>> {
        self.solved_repodata_records
            .get(environment)
            .and_then(|records| records.get(&platform))
            .map(|records| {
                let records = records.clone();
                Either::Left(async move { records.wait().await.clone() })
            })
            .or_else(|| {
                self.locked_repodata_records
                    .get(environment)
                    .and_then(|records| records.get(&platform))
                    .cloned()
                    .map(ready)
                    .map(Either::Right)
            })
    }

    /// Takes the latest repodata records for the given environment and platform. Returns `None` if
    /// neither the records exist nor are in the process of being updated.
    ///
    /// This function panics if the repodata records are still pending.
    pub fn take_latest_repodata_records(
        &mut self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<Vec<RepoDataRecord>> {
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

    /// Takes the latest pypi records for the given environment and platform. Returns `None` if
    /// neither the records exist nor are in the process of being updated.
    ///
    /// This function panics if the repodata records are still pending.
    pub fn take_latest_pypi_records(
        &mut self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<Vec<(PypiPackageData, PypiPackageEnvironmentData)>> {
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
            .map(|(env, cell)| {
                let prefix = Arc::into_inner(cell)
                    .expect("prefixes must not be shared")
                    .into_inner()
                    .expect("prefix must be available");
                (env, prefix)
            })
            .collect()
    }

    /// Returns a future that will resolve to the solved repodata records for the given environment
    /// or `None` if no task was spawned to instantiate the prefix.
    pub fn get_conda_prefix(
        &self,
        environment: &Environment<'p>,
    ) -> Option<impl Future<Output = (Prefix, PythonStatus)>> {
        let cell = self.instantiated_conda_prefixes.get(environment)?.clone();
        Some(async move { cell.wait().await.clone() })
    }
}

/// Ensures that the lock-file is up-to-date with the project.
///
/// This function will return a [`LockFileDerivedData`] struct that contains the lock-file and any
/// potential derived data that was computed as part of this function. The derived data might be
/// usable by other functions to avoid recomputing the same data.
///
/// This function starts by checking if the lock-file is up-to-date. If it is not up-to-date it will
/// construct a task graph of all the work that needs to be done to update the lock-file. The tasks
/// are awaited in a specific order to make sure that we can start instantiating prefixes as soon as
/// possible.
async fn ensure_up_to_date_lock_file(
    project: &Project,
    existing_repo_data: IndexMap<(Channel, Platform), SparseRepoData>,
    lock_file_usage: LockFileUsage,
    no_install: bool,
) -> miette::Result<LockFileDerivedData<'_>> {
    let lock_file = load_lock_file(project).await?;
    let current_platform = Platform::current();
    let package_cache = Arc::new(PackageCache::new(config::get_cache_dir()?.join("pkgs")));

    // should we check the lock-file in the first place?
    if !lock_file_usage.should_check_if_out_of_date() {
        tracing::info!("skipping check if lock-file is up-to-date");

        return Ok(LockFileDerivedData {
            lock_file,
            package_cache,
            repo_data: existing_repo_data,
            updated_conda_prefixes: Default::default(),
            updated_pypi_prefixes: Default::default(),
        });
    }

    // Check which environments are out of date.
    let outdated = OutdatedEnvironments::from_project_and_lock_file(project, &lock_file);
    if outdated.is_empty() {
        tracing::info!("the lock-file is up-to-date");

        // If no-environment is outdated we can return early.
        return Ok(LockFileDerivedData {
            lock_file,
            package_cache,
            repo_data: existing_repo_data,
            updated_conda_prefixes: Default::default(),
            updated_pypi_prefixes: Default::default(),
        });
    }

    // If the lock-file is out of date, but we're not allowed to update it, we should exit.
    if !lock_file_usage.allows_lock_file_updates() {
        miette::bail!("lock-file not up-to-date with the project");
    }

    // Determine the repodata that we're going to need to solve the environments. For all outdated
    // conda targets we take the union of all the channels that are used by the environment.
    //
    // The NoArch platform is always added regardless of whether it is explicitly used by the
    // environment.
    let mut fetch_targets = IndexSet::new();
    for (environment, platforms) in outdated.conda.iter() {
        for channel in environment.channels() {
            for platform in platforms {
                fetch_targets.insert((channel.clone(), *platform));
            }
            fetch_targets.insert((channel.clone(), Platform::NoArch));
        }
    }

    // Fetch all the repodata that we need to solve the environments.
    let mut repo_data = fetch_sparse_repodata_targets(
        fetch_targets
            .into_iter()
            .filter(|target| !existing_repo_data.contains_key(target)),
        project.authenticated_client(),
    )
    .await?;

    // Add repo data that was already fetched
    repo_data.extend(existing_repo_data);

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
                                .map(|(platform, records)| (platform, Arc::new(records)))
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
                            .map(|(platform, records)| (platform, Arc::new(records)))
                            .collect(),
                    )
                })
        })
        .collect::<HashMap<_, HashMap<_, _>>>();

    let mut context = UpdateContext {
        repo_data: Arc::new(repo_data),
        locked_repodata_records,
        locked_pypi_records,
        solved_repodata_records: HashMap::new(),
        instantiated_conda_prefixes: HashMap::new(),
        solved_pypi_records: HashMap::new(),
    };

    // This will keep track of all outstanding tasks that we need to wait for. All tasks are added
    // to this list after they are spawned. This function blocks until all pending tasks have either
    // completed or errored.
    let mut pending_futures = FuturesUnordered::new();

    // Spawn tasks for all the conda targets that are out of date.
    for (environment, platforms) in outdated.conda {
        // Turn the platforms into an IndexSet, so we have a little control over the order in which
        // we solve the platforms. We want to solve the current platform first, so we can start
        // instantiating prefixes if we have to.
        let mut ordered_platforms = platforms.into_iter().collect::<IndexSet<_>>();
        if let Some(current_platform_index) = ordered_platforms.get_index_of(&current_platform) {
            ordered_platforms.move_index(current_platform_index, 0);
        }

        for platform in ordered_platforms {
            // Extract the records from the existing lock file
            let existing_records = context
                .locked_repodata_records
                .get_mut(&environment)
                .and_then(|records| records.remove(&platform))
                .map(|records| Arc::try_unwrap(records).unwrap_or_else(|arc| (*arc).clone()))
                .unwrap_or_default();

            // Spawn a task to solve the environment.
            let conda_solve_task = spawn_solve_conda_environment_task(
                environment.clone(),
                existing_records,
                context.repo_data.clone(),
                platform,
            )
            .boxed_local();

            pending_futures.push(conda_solve_task);
            let previous_cell = context
                .solved_repodata_records
                .entry(environment.clone())
                .or_default()
                .insert(
                    platform,
                    Arc::new(BarrierCell::<Arc<Vec<RepoDataRecord>>>::new()),
                );
            assert!(
                previous_cell.is_none(),
                "a cell has already been added to update conda records"
            );
        }
    }

    // Spawn tasks to instantiate prefixes that we need to be able to solve Pypi packages.
    //
    // Solving Pypi packages requires a python interpreter to be present in the prefix, therefore we
    // first need to make sure we have conda packages available, then we can instantiate the
    // prefix with at least the required conda packages (including a python interpreter) and then
    // we can solve the Pypi packages using the installed interpreter.
    //
    // We only need to instantiate the prefix for the current platform.
    for (environment, platforms) in outdated.pypi.iter() {
        // Only instantiate a prefix if any of the platforms actually contain pypi dependencies. If
        // there are no pypi-dependencies than solving is also not required and thus a prefix is
        // also not required.
        if !platforms
            .iter()
            .any(|p| !environment.pypi_dependencies(Some(*p)).is_empty())
        {
            continue;
        }

        // If we are not allowed to install, we can't instantiate a prefix.
        if no_install {
            miette::bail!("Cannot update pypi dependencies without first installing a conda prefix that includes python.");
        }

        // Construct a future that will resolve when we have the repodata available for the current
        // platform for this environment.
        let records_future = context
            .get_latest_repodata_records(environment, current_platform)
            .expect("conda records should be available now or in the future");

        // Spawn a task to instantiate the environment
        let environment_name = environment.name().clone();
        let pypi_env_task =
            spawn_create_prefix_task(environment.clone(), package_cache.clone(), records_future)
                .map_err(move |e| {
                    e.context(format!(
                        "failed to instantiate a prefix for '{}'",
                        environment_name
                    ))
                })
                .boxed_local();

        pending_futures.push(pypi_env_task);
        context
            .instantiated_conda_prefixes
            .insert(environment.clone(), Arc::new(BarrierCell::new()));
    }

    // Spawn tasks to update the pypi packages.
    for (environment, platform) in outdated
        .pypi
        .into_iter()
        .flat_map(|(env, platforms)| platforms.into_iter().map(move |p| (env.clone(), p)))
    {
        let dependencies = environment.pypi_dependencies(Some(platform));
        let pypi_solve_task = if dependencies.is_empty() {
            // If there are no pypi dependencies we can skip solving the pypi packages.
            Either::Left(ready(Ok(TaskResult::PypiSolved(
                environment.name().clone(),
                platform,
                Vec::new(),
            ))))
        } else {
            // Construct a future that will resolve when we have the repodata available
            let repodata_future = context
                .get_latest_repodata_records(&environment, platform)
                .expect("conda records should be available now or in the future");

            // Construct a future that will resolve when we have the conda prefix available
            let prefix_future = context
                .get_conda_prefix(&environment)
                .expect("prefix should be available now or in the future");

            // Spawn a task to solve the pypi environment
            let pypi_solve_future = spawn_solve_pypi_task(
                environment.clone(),
                platform,
                repodata_future,
                prefix_future,
                SDistResolution::default(),
            );

            Either::Right(pypi_solve_future)
        };

        pending_futures.push(pypi_solve_task.boxed_local());
        let previous_cell = context
            .solved_pypi_records
            .entry(environment)
            .or_default()
            .insert(platform, Arc::new(BarrierCell::new()));
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
    // The spawned futures each result either in an error or in a `TaskResult`. The `TaskResult`
    // contains the result of the task. The results are stored into [`BarrierCell`]s which allows
    // other tasks to respond to the data becoming available.
    //
    // A loop on the main task is used versus individually spawning all tasks for two reasons:
    //
    // 1. This provides some control over when data is polled and broadcasted to other tasks. No
    //    data is broadcasted until we start polling futures here. This reduces the risk of
    //    race-conditions where data has already been broadcasted before a task subscribes to it.
    // 2. The futures stored in `pending_futures` do not necessarily have to be `'static`. Which
    //    makes them easier to work with.
    while let Some(result) = pending_futures.next().await {
        top_level_progress.inc(1);
        match result? {
            TaskResult::CondaSolved(environment, platform, records) => {
                let environment = project
                    .environment(&environment)
                    .expect("environment should exist");

                context
                    .solved_repodata_records
                    .get_mut(&environment)
                    .expect("the entry for this environment should exist")
                    .get_mut(&platform)
                    .expect("the entry for this platform should exist")
                    .set(Arc::new(records))
                    .expect("records should not be solved twice");

                tracing::info!(
                    "solved conda packages for '{}' '{}'",
                    environment.name().fancy_display(),
                    platform
                );
            }
            TaskResult::CondaPrefixUpdated(environment, prefix, python_status) => {
                let environment = project
                    .environment(&environment)
                    .expect("environment should exist");

                context
                    .instantiated_conda_prefixes
                    .get_mut(&environment)
                    .expect("the entry for this environment should exists")
                    .set((prefix, *python_status))
                    .expect("prefix should not be instantiated twice");

                tracing::info!(
                    "updated conda packages in the '{}' prefix",
                    environment.name().fancy_display()
                );
            }
            TaskResult::PypiSolved(environment, platform, records) => {
                let environment = project
                    .environment(&environment)
                    .expect("environment should exist");

                context
                    .solved_pypi_records
                    .get_mut(&environment)
                    .expect("the entry for this environment should exist")
                    .get_mut(&platform)
                    .expect("the entry for this platform should exist")
                    .set(Arc::new(records))
                    .expect("records should not be solved twice");

                tracing::info!(
                    "solved pypi packages for '{}' '{}'",
                    environment.name().fancy_display(),
                    platform
                );
            }
        }
    }

    // Construct a new lock-file containing all the updated or old records.
    let mut builder = LockFile::builder();

    // Iterate over all environments and add their records to the lock-file.
    for environment in project.environments() {
        builder.set_channels(
            environment.name().as_str(),
            environment
                .channels()
                .into_iter()
                .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string())),
        );

        for platform in environment.platforms() {
            if let Some(records) = context.take_latest_repodata_records(&environment, platform) {
                for record in records {
                    builder.add_conda_package(environment.name().as_str(), platform, record.into());
                }
            }
            if let Some(records) = context.take_latest_pypi_records(&environment, platform) {
                for (pkg_data, pkg_env_data) in records {
                    builder.add_pypi_package(
                        environment.name().as_str(),
                        platform,
                        pkg_data,
                        pkg_env_data,
                    );
                }
            }
        }
    }

    // Store the lock file
    let lock_file = builder.finish();
    lock_file
        .to_path(&project.lock_file_path())
        .into_diagnostic()
        .context("failed to write lock-file to disk")?;

    top_level_progress.finish_and_clear();

    Ok(LockFileDerivedData {
        lock_file,
        package_cache,
        updated_conda_prefixes: context.take_instantiated_conda_prefixes(),
        updated_pypi_prefixes: HashMap::default(),
        repo_data: Arc::into_inner(context.repo_data)
            .expect("repo data should not be shared anymore"),
    })
}

/// Represents data that is sent back from a task. This is used to communicate the result of a task
/// back to the main task which will forward the information to other tasks waiting for results.
enum TaskResult {
    CondaSolved(EnvironmentName, Platform, Vec<RepoDataRecord>),
    CondaPrefixUpdated(EnvironmentName, Prefix, Box<PythonStatus>),
    PypiSolved(
        EnvironmentName,
        Platform,
        Vec<(PypiPackageData, PypiPackageEnvironmentData)>,
    ),
}

/// A task that solves the conda dependencies for a given environment.
async fn spawn_solve_conda_environment_task(
    environment: Environment<'_>,
    existing_repodata_records: Vec<RepoDataRecord>,
    sparse_repo_data: Arc<IndexMap<(Channel, Platform), SparseRepoData>>,
    platform: Platform,
) -> miette::Result<TaskResult> {
    // Get the dependencies for this platform
    let dependencies = environment.dependencies(None, Some(platform));

    // Get the virtual packages for this platform
    let virtual_packages = environment.virtual_packages(platform);

    // Get the environment name
    let environment_name = environment.name().clone();

    // The list of channels and platforms we need for this task
    let channels = environment.channels().into_iter().cloned().collect_vec();

    // Capture local variables
    let sparse_repo_data = sparse_repo_data.clone();

    // Whether there are pypi dependencies, and we should fetch purls.
    let has_pypi_dependencies = environment.has_pypi_dependencies();

    tokio::spawn(async move {
        let pb = SolveProgressBar::new(
            global_multi_progress().add(ProgressBar::hidden()),
            platform,
            environment_name.clone(),
        );
        pb.start();

        // Convert the dependencies into match specs
        let match_specs = dependencies
            .iter_specs()
            .map(|(name, constraint)| {
                MatchSpec::from_nameless(constraint.clone(), Some(name.clone()))
            })
            .collect_vec();

        // Extract the package names from the dependencies
        let package_names = dependencies.names().cloned().collect_vec();

        // Extract the repo data records needed to solve the environment.
        pb.set_message("loading repodata");
        let available_packages = load_sparse_repo_data_async(
            package_names.clone(),
            sparse_repo_data,
            channels,
            platform,
        )
        .await?;

        // Solve conda packages
        pb.set_message("resolving conda");
        let mut records = lock_file::resolve_conda_dependencies(
            match_specs,
            virtual_packages,
            existing_repodata_records,
            available_packages,
        )?;

        // Add purl's for the conda packages that are also available as pypi packages if we need them.
        if has_pypi_dependencies {
            lock_file::pypi::amend_pypi_purls(&mut records).await?;
        }

        // Finish the progress bar
        pb.finish();

        Ok(TaskResult::CondaSolved(environment_name, platform, records))
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })
}

/// A task that solves the pypi dependencies for a given environment.
async fn spawn_solve_pypi_task(
    environment: Environment<'_>,
    platform: Platform,
    repodata_records: impl Future<Output = Arc<Vec<RepoDataRecord>>>,
    prefix: impl Future<Output = (Prefix, PythonStatus)>,
    sdist_resolution: SDistResolution,
) -> miette::Result<TaskResult> {
    // Get the Pypi dependencies for this environment
    let dependencies = environment.pypi_dependencies(Some(platform));
    if dependencies.is_empty() {
        return Ok(TaskResult::PypiSolved(
            environment.name().clone(),
            platform,
            Vec::new(),
        ));
    }

    // Get the system requirements for this environment
    let system_requirements = environment.system_requirements();

    // Get the package database
    let package_db = environment.project().pypi_package_db()?;

    // Wait until the conda records and prefix are available.
    let (repodata_records, (prefix, python_status)) = tokio::join!(repodata_records, prefix);

    let environment_name = environment.name().clone();
    let pypi_packages = tokio::spawn(async move {
        let pb = SolveProgressBar::new(
            global_multi_progress().add(ProgressBar::hidden()),
            platform,
            environment_name,
        );
        pb.start();

        let result = resolve_pypi(
            &package_db,
            dependencies,
            system_requirements,
            &repodata_records,
            &[],
            platform,
            &pb.pb,
            python_status
                .location()
                .map(|path| prefix.root().join(path))
                .as_deref(),
            sdist_resolution,
        )
        .await;

        pb.finish();

        result
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })?;

    Ok(TaskResult::PypiSolved(
        environment.name().clone(),
        platform,
        pypi_packages,
    ))
}

/// Updates the prefix for the given environment.
///
/// This function will wait until the conda records for the prefix are available.
async fn spawn_create_prefix_task(
    environment: Environment<'_>,
    package_cache: Arc<PackageCache>,
    conda_records: impl Future<Output = Arc<Vec<RepoDataRecord>>>,
) -> miette::Result<TaskResult> {
    let environment_name = environment.name().clone();
    let prefix = Prefix::new(environment.dir());
    let client = environment.project().authenticated_client().clone();

    // Spawn a task to determine the currently installed packages.
    let installed_packages_future = tokio::spawn({
        let prefix = prefix.clone();
        async move { prefix.find_installed_packages(None).await }
    })
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    });

    // Wait until the conda records are available and until the installed packages for this prefix
    // are available.
    let (conda_records, installed_packages) =
        tokio::try_join!(conda_records.map(Ok), installed_packages_future)?;

    // Spawn a background task to update the prefix
    let python_status = tokio::spawn({
        let prefix = prefix.clone();
        let environment_name = environment_name.clone();
        async move {
            update_prefix_conda(
                environment_name.as_str(),
                &prefix,
                package_cache,
                client,
                installed_packages,
                &conda_records,
                Platform::current(),
            )
            .await
        }
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })?;

    Ok(TaskResult::CondaPrefixUpdated(
        environment_name,
        prefix,
        Box::new(python_status),
    ))
}

/// Load the repodata records for the specified platform and package names in the background. This
/// is a CPU and IO intensive task so we run it in a blocking task to not block the main task.
pub async fn load_sparse_repo_data_async(
    package_names: Vec<PackageName>,
    sparse_repo_data: Arc<IndexMap<(Channel, Platform), SparseRepoData>>,
    channels: Vec<Channel>,
    platform: Platform,
) -> miette::Result<Vec<Vec<RepoDataRecord>>> {
    tokio::task::spawn_blocking(move || {
        let sparse = channels
            .into_iter()
            .cartesian_product(vec![platform, Platform::NoArch])
            .filter_map(|target| sparse_repo_data.get(&target));

        // Load only records we need for this platform
        SparseRepoData::load_records_recursive(sparse, package_names, None).into_diagnostic()
    })
    .await
    .map_err(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => miette::miette!("the operation was cancelled"),
    })
    .map_or_else(Err, identity)
    .with_context(|| {
        format!(
            "failed to load repodata records for platform '{}'",
            platform.as_str()
        )
    })
}

/// A helper struct that manages a progress-bar for solving an environment.
#[derive(Clone)]
pub(crate) struct SolveProgressBar {
    pb: ProgressBar,
    platform: Platform,
    environment_name: EnvironmentName,
}

impl SolveProgressBar {
    pub fn new(pb: ProgressBar, platform: Platform, environment_name: EnvironmentName) -> Self {
        pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "   ({:>12}) {:<9} ..",
                environment_name.fancy_display(),
                platform.to_string(),
            ))
            .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        Self {
            pb,
            platform,
            environment_name,
        }
    }

    pub fn start(&self) {
        self.pb.reset_elapsed();
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {{spinner:.dim}} {:>12}: {:<9} [{{elapsed_precise}}] {{msg:.dim}}",
                self.environment_name.fancy_display(),
                self.platform.to_string(),
            ))
            .unwrap(),
        );
    }

    pub fn set_message(&self, msg: impl Into<Cow<'static, str>>) {
        self.pb.set_message(msg);
    }

    pub fn finish(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {} ({:>12}) {:<9} [{{elapsed_precise}}]",
                console::style(console::Emoji("✔", "↳")).green(),
                self.environment_name.fancy_display(),
                self.platform.to_string(),
            ))
            .unwrap(),
        );
        self.pb.finish_and_clear();
    }
}
