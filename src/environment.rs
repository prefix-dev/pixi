use miette::{Context, IntoDiagnostic};

use crate::lock_file::resolve_pypi;
use crate::project::manifest::PyPiRequirement;
use crate::project::virtual_packages::get_minimal_virtual_packages;
use crate::project::{Dependencies, SolveGroup};
use crate::{
    config, consts, install, install_pypi, lock_file,
    lock_file::{
        load_lock_file, verify_environment_satisfiability, verify_platform_satisfiability,
        PlatformUnsat, PypiPackageIdentifier,
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
    Project, SpecType,
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
    Channel, GenericVirtualPackage, MatchSpec, PackageName, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_lock::{LockFile, Package, PypiPackageData, PypiPackageEnvironmentData};
use rattler_repodata_gateway::sparse::SparseRepoData;
use reqwest_middleware::ClientWithMiddleware;
use rip::{index::PackageDb, resolve::SDistResolution};
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::hash::Hash;
use std::path::PathBuf;
use std::str::FromStr;
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
    environment_name: &EnvironmentName,
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
    progress::await_in_progress(
        format!(
            "updating pypi package in '{}'",
            environment_name.fancy_display()
        ),
        |_| {
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
        },
    )
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
    environment_name: GroupedEnvironmentName,
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
        progress::await_in_progress(
            format!(
                "updating packages in '{}'",
                environment_name.fancy_display()
            ),
            |pb| async {
                install::execute_transaction(
                    package_cache,
                    &transaction,
                    &installed_packages,
                    prefix.root().to_path_buf(),
                    authenticated_client,
                    pb,
                )
                .await
            },
        )
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
            environment.name(),
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
            GroupedEnvironmentName::Environment(environment.name().clone()),
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

        // Determine which solve-groups are out of date.
        let mut conda_solve_groups_out_of_date = HashMap::new();
        let mut pypi_solve_groups_out_of_date = HashMap::new();
        for (environment, platforms) in &outdated_conda {
            let Some(solve_group) = environment.solve_group() else {
                continue;
            };
            conda_solve_groups_out_of_date
                .entry(solve_group)
                .or_insert_with(HashSet::new)
                .extend(platforms.iter().copied());
        }
        for (environment, platforms) in &outdated_pypi {
            let Some(solve_group) = environment.solve_group() else {
                continue;
            };
            pypi_solve_groups_out_of_date
                .entry(solve_group)
                .or_insert_with(HashSet::new)
                .extend(platforms.iter().copied());
        }

        // Check solve-groups, all environments in the same solve group must share the same
        // dependencies.
        for solve_group in project.solve_groups() {
            for platform in solve_group
                .environments()
                .flat_map(|env| env.platforms())
                .unique()
            {
                // Keep track of if any of the package types are out of date
                let mut conda_package_mismatch = false;
                let mut pypi_package_mismatch = false;

                // Keep track of the packages by name to check for mismatches between environments.
                let mut conda_packages_by_name = HashMap::new();
                let mut pypi_packages_by_name = HashMap::new();

                // Iterate over all environments to compare the packages.
                for env in solve_group.environments() {
                    if outdated_conda
                        .get(&env)
                        .and_then(|p| p.get(&platform))
                        .is_some()
                    {
                        // If the environment is already out-of-date there is no need to check it,
                        // because the solve-group is already out-of-date.
                        break;
                    }

                    let Some(locked_env) = lock_file.environment(env.name().as_str()) else {
                        // If the environment is missing, we already marked it as out of date.
                        continue;
                    };

                    for package in locked_env.packages(platform).into_iter().flatten() {
                        match package {
                            Package::Conda(pkg) => {
                                match conda_packages_by_name.get(&pkg.package_record().name) {
                                    None => {
                                        conda_packages_by_name.insert(
                                            pkg.package_record().name.clone(),
                                            pkg.url().clone(),
                                        );
                                    }
                                    Some(url) if pkg.url() != url => {
                                        conda_package_mismatch = true;
                                    }
                                    _ => {}
                                }
                            }
                            Package::Pypi(pkg) => {
                                match pypi_packages_by_name.get(&pkg.data().package.name) {
                                    None => {
                                        pypi_packages_by_name.insert(
                                            pkg.data().package.name.clone(),
                                            pkg.url().clone(),
                                        );
                                    }
                                    Some(url) if pkg.url() != url => {
                                        pypi_package_mismatch = true;
                                    }
                                    _ => {}
                                }
                            }
                        }

                        // If there is a conda package mismatch there is also a pypi mismatch and we
                        // can break early.
                        if conda_package_mismatch {
                            pypi_package_mismatch = true;
                            break;
                        }
                    }

                    // If there is a conda package mismatch there is also a pypi mismatch and we can
                    // break early.
                    if conda_package_mismatch {
                        pypi_package_mismatch = true;
                        break;
                    }
                }

                // If there is a mismatch there is a mismatch for the entire group
                if conda_package_mismatch {
                    conda_solve_groups_out_of_date
                        .entry(solve_group.clone())
                        .or_default()
                        .insert(platform);
                }

                if pypi_package_mismatch {
                    pypi_solve_groups_out_of_date
                        .entry(solve_group.clone())
                        .or_default()
                        .insert(platform);
                }
            }
        }

        // Mark the rest of the environments out of date for all solve groups
        for (solve_group, platforms) in conda_solve_groups_out_of_date {
            for env in solve_group.environments() {
                outdated_conda
                    .entry(env.clone())
                    .or_default()
                    .extend(platforms.iter().copied());
            }
        }

        for (solve_group, platforms) in pypi_solve_groups_out_of_date {
            for env in solve_group.environments() {
                outdated_pypi
                    .entry(env.clone())
                    .or_default()
                    .extend(platforms.iter().copied());
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
type PerGroup<'p, T> = HashMap<GroupedEnvironment<'p>, T>;
type PerEnvironmentAndPlatform<'p, T> = PerEnvironment<'p, HashMap<Platform, T>>;
type PerGroupAndPlatform<'p, T> = PerGroup<'p, HashMap<Platform, T>>;

#[derive(Default)]
struct UpdateContext<'p> {
    /// Repodata that is available to the solve tasks.
    repo_data: Arc<IndexMap<(Channel, Platform), SparseRepoData>>,

    /// Repodata records from the lock-file. This contains the records that actually exist in the
    /// lock-file. If the lock-file is missing or partially missing then the data also won't exist
    /// in this field.
    locked_repodata_records: PerEnvironmentAndPlatform<'p, Arc<RepoDataRecordsByName>>,

    /// Repodata records from the lock-file grouped by solve-group.
    locked_grouped_repodata_records: PerGroupAndPlatform<'p, Arc<RepoDataRecordsByName>>,

    /// Repodata records from the lock-file. This contains the records that actually exist in the
    /// lock-file. If the lock-file is missing or partially missing then the data also won't exist
    /// in this field.
    locked_pypi_records: PerEnvironmentAndPlatform<'p, Arc<PypiRecordsByName>>,

    /// Keeps track of all pending conda targets that are being solved. The mapping contains a
    /// [`BarrierCell`] that will eventually contain the solved records computed by another task.
    /// This allows tasks to wait for the records to be solved before proceeding.
    solved_repodata_records:
        PerEnvironmentAndPlatform<'p, Arc<BarrierCell<Arc<RepoDataRecordsByName>>>>,

    /// Keeps track of all pending grouped conda targets that are being solved.
    grouped_solved_repodata_records:
        PerGroupAndPlatform<'p, Arc<BarrierCell<Arc<RepoDataRecordsByName>>>>,

    /// Keeps track of all pending prefix updates. This only tracks the conda updates to a prefix,
    /// not whether the pypi packages have also been updated.
    instantiated_conda_prefixes: PerGroup<'p, Arc<BarrierCell<(Prefix, PythonStatus)>>>,

    /// Keeps track of all pending conda targets that are being solved. The mapping contains a
    /// [`BarrierCell`] that will eventually contain the solved records computed by another task.
    /// This allows tasks to wait for the records to be solved before proceeding.
    solved_pypi_records: PerEnvironmentAndPlatform<'p, Arc<BarrierCell<Arc<PypiRecordsByName>>>>,

    /// Keeps track of all pending grouped pypi targets that are being solved.
    grouped_solved_pypi_records: PerGroupAndPlatform<'p, Arc<BarrierCell<Arc<PypiRecordsByName>>>>,
}

impl<'p> UpdateContext<'p> {
    /// Returns a future that will resolve to the solved repodata records for the given environment
    /// or `None` if the records do not exist and are also not in the process of being updated.
    pub fn get_latest_repodata_records(
        &self,
        environment: &Environment<'p>,
        platform: Platform,
    ) -> Option<impl Future<Output = Arc<RepoDataRecordsByName>>> {
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

    /// Returns a future that will resolve to the solved repodata records for the given environment
    /// group or `None` if the records do not exist and are also not in the process of being
    /// updated.
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

    /// Takes the latest repodata records for the given environment and platform. Returns `None` if
    /// neither the records exist nor are in the process of being updated.
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

    /// Takes the latest pypi records for the given environment and platform. Returns `None` if
    /// neither the records exist nor are in the process of being updated.
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

    /// Returns a future that will resolve to the solved repodata records for the given environment
    /// or `None` if no task was spawned to instantiate the prefix.
    pub fn get_conda_prefix(
        &self,
        environment: &GroupedEnvironment<'p>,
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

    // For every grouped environment extract the data from the lock-file. If multiple environments in a single
    // solve-group have different versions for a single package name than the record with the highest version is used.
    // This logic is implemented in `RepoDataRecordsByName::from_iter`. This can happen if previously two environments
    // did not share the same solve-group.
    let locked_grouped_repodata_records = all_grouped_environments
        .iter()
        .filter_map(|group| {
            let records = match group {
                GroupedEnvironment::Environment(env) => locked_repodata_records.get(env)?.clone(),
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

    let mut context = UpdateContext {
        repo_data: Arc::new(repo_data),

        locked_repodata_records,
        locked_grouped_repodata_records,
        locked_pypi_records,

        solved_repodata_records: HashMap::new(),
        instantiated_conda_prefixes: HashMap::new(),
        solved_pypi_records: HashMap::new(),
        grouped_solved_repodata_records: HashMap::new(),
        grouped_solved_pypi_records: HashMap::new(),
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

        // Determine the source of the solve information
        let source = GroupedEnvironment::from(environment.clone());

        for platform in ordered_platforms {
            // Is there an existing pending task to solve the group?
            let group_solve_records = if let Some(cell) = context
                .grouped_solved_repodata_records
                .get(&source)
                .and_then(|platforms| platforms.get(&platform))
            {
                // Yes, we can reuse the existing cell.
                cell.clone()
            } else {
                // No, we need to spawn a task to update for the entire solve group.
                let locked_group_records = context
                    .locked_grouped_repodata_records
                    .get(&source)
                    .and_then(|records| records.get(&platform))
                    .cloned()
                    .unwrap_or_default();

                // Spawn a task to solve the group.
                let group_solve_task = spawn_solve_conda_environment_task(
                    source.clone(),
                    locked_group_records,
                    context.repo_data.clone(),
                    platform,
                )
                .boxed_local();

                // Store the task so we can poll it later.
                pending_futures.push(group_solve_task);

                // Create an entry that can be used by other tasks to wait for the result.
                let cell = Arc::new(BarrierCell::new());
                let previous_cell = context
                    .grouped_solved_repodata_records
                    .entry(source.clone())
                    .or_default()
                    .insert(platform, cell.clone());
                assert!(
                    previous_cell.is_none(),
                    "a cell has already been added to update conda records"
                );

                cell
            };

            // Spawn a task to extract the records from the group solve task.
            let records_future =
                spawn_extract_conda_environment_task(environment.clone(), platform, async move {
                    group_solve_records.wait().await.clone()
                })
                .boxed_local();

            pending_futures.push(records_future);
            let previous_cell = context
                .solved_repodata_records
                .entry(environment.clone())
                .or_default()
                .insert(platform, Arc::default());
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

        // Check if the group is already being instantiated
        let group = GroupedEnvironment::from(environment.clone());
        if context.instantiated_conda_prefixes.contains_key(&group) {
            continue;
        }

        // Construct a future that will resolve when we have the repodata available for the current
        // platform for this group.
        let records_future = context
            .get_latest_group_repodata_records(&group, current_platform)
            .expect("conda records should be available now or in the future");

        // Spawn a task to instantiate the environment
        let environment_name = environment.name().clone();
        let pypi_env_task =
            spawn_create_prefix_task(group.clone(), package_cache.clone(), records_future)
                .map_err(move |e| {
                    e.context(format!(
                        "failed to instantiate a prefix for '{}'",
                        environment_name
                    ))
                })
                .boxed_local();

        pending_futures.push(pypi_env_task);
        let previous_cell = context
            .instantiated_conda_prefixes
            .insert(group, Arc::new(BarrierCell::new()));
        assert!(
            previous_cell.is_none(),
            "cannot update the same group twice"
        )
    }

    // Spawn tasks to update the pypi packages.
    for (environment, platform) in outdated
        .pypi
        .into_iter()
        .flat_map(|(env, platforms)| platforms.into_iter().map(move |p| (env.clone(), p)))
    {
        let dependencies = environment.pypi_dependencies(Some(platform));
        if dependencies.is_empty() {
            pending_futures.push(
                ready(Ok(TaskResult::PypiSolved(
                    environment.name().clone(),
                    platform,
                    Arc::default(),
                )))
                .boxed_local(),
            );
        } else {
            let group = GroupedEnvironment::from(environment.clone());

            // Solve all the pypi records in the solve group together.
            let grouped_pypi_records = if let Some(cell) = context
                .grouped_solved_pypi_records
                .get(&group)
                .and_then(|records| records.get(&platform))
            {
                // There is already a task to solve the pypi records for the group.
                cell.clone()
            } else {
                // Construct a future that will resolve when we have the repodata available
                let repodata_future = context
                    .get_latest_group_repodata_records(&group, platform)
                    .expect("conda records should be available now or in the future");

                // Construct a future that will resolve when we have the conda prefix available
                let prefix_future = context
                    .get_conda_prefix(&group)
                    .expect("prefix should be available now or in the future");

                // Spawn a task to solve the pypi environment
                let pypi_solve_future = spawn_solve_pypi_task(
                    group.clone(),
                    platform,
                    repodata_future,
                    prefix_future,
                    SDistResolution::default(),
                );

                pending_futures.push(pypi_solve_future.boxed_local());

                let cell = Arc::new(BarrierCell::new());
                let previous_cell = context
                    .grouped_solved_pypi_records
                    .entry(group)
                    .or_default()
                    .insert(platform, cell.clone());
                assert!(
                    previous_cell.is_none(),
                    "a cell has already been added to update pypi records"
                );

                cell
            };

            // Followed by spawning a task to extract exactly the pypi records that are needed for
            // this environment.
            let pypi_records_future = async move { grouped_pypi_records.wait().await.clone() };
            let conda_records_future = context
                .get_latest_repodata_records(&environment, platform)
                .expect("must have conda records available");
            let records_future = spawn_extract_pypi_environment_task(
                environment.clone(),
                platform,
                conda_records_future,
                pypi_records_future,
            )
            .boxed_local();
            pending_futures.push(records_future);
        }

        let previous_cell = context
            .solved_pypi_records
            .entry(environment)
            .or_default()
            .insert(platform, Arc::default());
        assert!(
            previous_cell.is_none(),
            "a cell has already been added to extract pypi records"
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
            TaskResult::CondaGroupSolved(group_name, platform, records) => {
                let group = GroupedEnvironment::from_name(project, &group_name)
                    .expect("group should exist");

                context
                    .grouped_solved_repodata_records
                    .get_mut(&group)
                    .expect("the entry for this environment should exist")
                    .get_mut(&platform)
                    .expect("the entry for this platform should exist")
                    .set(Arc::new(records))
                    .expect("records should not be solved twice");

                match group_name {
                    GroupedEnvironmentName::Group(_) => {
                        tracing::info!(
                            "solved conda package for solve group '{}' '{}'",
                            group_name.fancy_display(),
                            platform
                        );
                    }
                    GroupedEnvironmentName::Environment(env_name) => {
                        tracing::info!(
                            "solved conda package for environment '{}' '{}'",
                            env_name.fancy_display(),
                            platform
                        );
                    }
                }
            }
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
                    .set(records)
                    .expect("records should not be solved twice");

                tracing::info!(
                    "extracted conda packages for '{}' '{}'",
                    environment.name().fancy_display(),
                    platform
                );
            }
            TaskResult::CondaPrefixUpdated(group_name, prefix, python_status) => {
                let group = GroupedEnvironment::from_name(project, &group_name)
                    .expect("grouped environment should exist");

                context
                    .instantiated_conda_prefixes
                    .get_mut(&group)
                    .expect("the entry for this environment should exists")
                    .set((prefix, *python_status))
                    .expect("prefix should not be instantiated twice");

                tracing::info!(
                    "updated conda packages in the '{}' prefix",
                    group.name().fancy_display()
                );
            }
            TaskResult::PypiGroupSolved(group_name, platform, records) => {
                let group = GroupedEnvironment::from_name(project, &group_name)
                    .expect("group should exist");

                context
                    .grouped_solved_pypi_records
                    .get_mut(&group)
                    .expect("the entry for this environment should exist")
                    .get_mut(&platform)
                    .expect("the entry for this platform should exist")
                    .set(Arc::new(records))
                    .expect("records should not be solved twice");

                match group_name {
                    GroupedEnvironmentName::Group(_) => {
                        tracing::info!(
                            "solved pypi package for solve group '{}' '{}'",
                            group_name.fancy_display(),
                            platform
                        );
                    }
                    GroupedEnvironmentName::Environment(env_name) => {
                        tracing::info!(
                            "solved pypi package for environment '{}' '{}'",
                            env_name.fancy_display(),
                            platform
                        );
                    }
                }
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
                    .set(records)
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
                for record in records.into_inner() {
                    builder.add_conda_package(environment.name().as_str(), platform, record.into());
                }
            }
            if let Some(records) = context.take_latest_pypi_records(&environment, platform) {
                for (pkg_data, pkg_env_data) in records.into_inner() {
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
    CondaGroupSolved(GroupedEnvironmentName, Platform, RepoDataRecordsByName),
    CondaSolved(EnvironmentName, Platform, Arc<RepoDataRecordsByName>),
    CondaPrefixUpdated(GroupedEnvironmentName, Prefix, Box<PythonStatus>),
    PypiGroupSolved(GroupedEnvironmentName, Platform, PypiRecordsByName),
    PypiSolved(EnvironmentName, Platform, Arc<PypiRecordsByName>),
}

/// A task that solves the conda dependencies for a given environment.
async fn spawn_solve_conda_environment_task(
    group: GroupedEnvironment<'_>,
    existing_repodata_records: Arc<RepoDataRecordsByName>,
    sparse_repo_data: Arc<IndexMap<(Channel, Platform), SparseRepoData>>,
    platform: Platform,
) -> miette::Result<TaskResult> {
    // Get the dependencies for this platform
    let dependencies = group.dependencies(None, Some(platform));

    // Get the virtual packages for this platform
    let virtual_packages = group.virtual_packages(platform);

    // Get the environment name
    let group_name = group.name();

    // The list of channels and platforms we need for this task
    let channels = group.channels().into_iter().cloned().collect_vec();

    // Capture local variables
    let sparse_repo_data = sparse_repo_data.clone();

    // Whether there are pypi dependencies, and we should fetch purls.
    let has_pypi_dependencies = group.has_pypi_dependencies();

    tokio::spawn(async move {
        let pb = SolveProgressBar::new(
            global_multi_progress().add(ProgressBar::hidden()),
            platform,
            group_name.clone(),
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
            existing_repodata_records.records.clone(),
            available_packages,
        )?;

        // Add purl's for the conda packages that are also available as pypi packages if we need them.
        if has_pypi_dependencies {
            lock_file::pypi::amend_pypi_purls(&mut records).await?;
        }

        // Turn the records into a map by name
        let records_by_name = RepoDataRecordsByName::from(records);

        // Finish the progress bar
        pb.finish();

        Ok(TaskResult::CondaGroupSolved(
            group_name,
            platform,
            records_by_name,
        ))
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })
}

/// Distill the repodata that is applicable for the given `environment` from the repodata of an entire solve group.
async fn spawn_extract_conda_environment_task(
    environment: Environment<'_>,
    platform: Platform,
    solve_group_records: impl Future<Output = Arc<RepoDataRecordsByName>>,
) -> miette::Result<TaskResult> {
    let group = GroupedEnvironment::from(environment.clone());

    // Await the records from the group
    let group_records = solve_group_records.await;

    // If the group is just the environment on its own we can immediately return the records.
    let records = match group {
        GroupedEnvironment::Environment(_) => {
            // For a single environment group we can just clone the Arc
            group_records.clone()
        }
        GroupedEnvironment::Group(_) => {
            let virtual_package_names = group
                .virtual_packages(platform)
                .into_iter()
                .map(|vp| vp.name)
                .collect::<HashSet<_>>();

            let environment_dependencies = environment.dependencies(None, Some(platform));
            Arc::new(group_records.subset(
                environment_dependencies.into_iter().map(|(name, _)| name),
                &virtual_package_names,
            ))
        }
    };

    Ok(TaskResult::CondaSolved(
        environment.name().clone(),
        platform,
        records,
    ))
}

async fn spawn_extract_pypi_environment_task(
    environment: Environment<'_>,
    platform: Platform,
    conda_records: impl Future<Output = Arc<RepoDataRecordsByName>>,
    solve_group_records: impl Future<Output = Arc<PypiRecordsByName>>,
) -> miette::Result<TaskResult> {
    let group = GroupedEnvironment::from(environment.clone());
    let dependencies = environment.pypi_dependencies(Some(platform));

    let records = match group {
        GroupedEnvironment::Environment(_) => {
            // For a single environment group we can just clone the Arc.
            solve_group_records.await.clone()
        }
        GroupedEnvironment::Group(_) => {
            // Convert all the conda records to package identifiers.
            let conda_package_identifiers = conda_records
                .await
                .records
                .iter()
                .filter_map(|record| PypiPackageIdentifier::from_record(record).ok())
                .flatten()
                .map(|identifier| (identifier.name.clone().into(), identifier))
                .collect::<HashMap<_, _>>();

            Arc::new(
                solve_group_records
                    .await
                    .subset(dependencies.into_keys(), &conda_package_identifiers),
            )
        }
    };

    Ok(TaskResult::PypiSolved(
        environment.name().clone(),
        platform,
        records,
    ))
}

/// A task that solves the pypi dependencies for a given environment.
async fn spawn_solve_pypi_task(
    environment: GroupedEnvironment<'_>,
    platform: Platform,
    repodata_records: impl Future<Output = Arc<RepoDataRecordsByName>>,
    prefix: impl Future<Output = (Prefix, PythonStatus)>,
    sdist_resolution: SDistResolution,
) -> miette::Result<TaskResult> {
    // Get the Pypi dependencies for this environment
    let dependencies = environment.pypi_dependencies(Some(platform));
    if dependencies.is_empty() {
        return Ok(TaskResult::PypiGroupSolved(
            environment.name().clone(),
            platform,
            PypiRecordsByName::default(),
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
            &repodata_records.records,
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

        result.map(PypiRecordsByName::from_iter)
    })
    .await
    .unwrap_or_else(|e| match e.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_err) => Err(miette::miette!("the operation was cancelled")),
    })?;

    Ok(TaskResult::PypiGroupSolved(
        environment.name().clone(),
        platform,
        pypi_packages,
    ))
}

/// Updates the prefix for the given environment.
///
/// This function will wait until the conda records for the prefix are available.
async fn spawn_create_prefix_task(
    group: GroupedEnvironment<'_>,
    package_cache: Arc<PackageCache>,
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

    // Wait until the conda records are available and until the installed packages for this prefix
    // are available.
    let (conda_records, installed_packages) =
        tokio::try_join!(conda_records.map(Ok), installed_packages_future)?;

    // Spawn a background task to update the prefix
    let python_status = tokio::spawn({
        let prefix = prefix.clone();
        let group_name = group_name.clone();
        async move {
            update_prefix_conda(
                group_name,
                &prefix,
                package_cache,
                client,
                installed_packages,
                &conda_records.records,
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
        group_name,
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
    environment_name: GroupedEnvironmentName,
}

impl SolveProgressBar {
    pub fn new(
        pb: ProgressBar,
        platform: Platform,
        environment_name: GroupedEnvironmentName,
    ) -> Self {
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

/// Either a solve group or an individual environment without a solve group.
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub enum GroupedEnvironment<'p> {
    Group(SolveGroup<'p>),
    Environment(Environment<'p>),
}

#[derive(Clone)]
pub enum GroupedEnvironmentName {
    Group(String),
    Environment(EnvironmentName),
}

impl GroupedEnvironmentName {
    pub fn fancy_display(&self) -> console::StyledObject<&str> {
        match self {
            GroupedEnvironmentName::Group(name) => console::style(name.as_str()).magenta(),
            GroupedEnvironmentName::Environment(name) => name.fancy_display(),
        }
    }
}

impl<'p> From<SolveGroup<'p>> for GroupedEnvironment<'p> {
    fn from(source: SolveGroup<'p>) -> Self {
        GroupedEnvironment::Group(source)
    }
}

impl<'p> From<Environment<'p>> for GroupedEnvironment<'p> {
    fn from(source: Environment<'p>) -> Self {
        source.solve_group().map_or_else(
            || GroupedEnvironment::Environment(source),
            GroupedEnvironment::Group,
        )
    }
}

impl<'p> GroupedEnvironment<'p> {
    pub fn from_name(project: &'p Project, name: &GroupedEnvironmentName) -> Option<Self> {
        match name {
            GroupedEnvironmentName::Group(g) => {
                Some(GroupedEnvironment::Group(project.solve_group(g)?))
            }
            GroupedEnvironmentName::Environment(env) => {
                Some(GroupedEnvironment::Environment(project.environment(env)?))
            }
        }
    }

    pub fn project(&self) -> &'p Project {
        match self {
            GroupedEnvironment::Group(group) => group.project(),
            GroupedEnvironment::Environment(env) => env.project(),
        }
    }

    pub fn prefix(&self) -> Prefix {
        Prefix::new(self.dir())
    }

    pub fn dir(&self) -> PathBuf {
        match self {
            GroupedEnvironment::Group(solve_group) => solve_group.dir(),
            GroupedEnvironment::Environment(env) => env.dir(),
        }
    }

    pub fn name(&self) -> GroupedEnvironmentName {
        match self {
            GroupedEnvironment::Group(group) => {
                GroupedEnvironmentName::Group(group.name().to_string())
            }
            GroupedEnvironment::Environment(env) => {
                GroupedEnvironmentName::Environment(env.name().clone())
            }
        }
    }

    pub fn dependencies(&self, kind: Option<SpecType>, platform: Option<Platform>) -> Dependencies {
        match self {
            GroupedEnvironment::Group(group) => group.dependencies(kind, platform),
            GroupedEnvironment::Environment(env) => env.dependencies(kind, platform),
        }
    }

    pub fn pypi_dependencies(
        &self,
        platform: Option<Platform>,
    ) -> IndexMap<rip::types::PackageName, Vec<PyPiRequirement>> {
        match self {
            GroupedEnvironment::Group(group) => group.pypi_dependencies(platform),
            GroupedEnvironment::Environment(env) => env.pypi_dependencies(platform),
        }
    }

    pub fn system_requirements(&self) -> SystemRequirements {
        match self {
            GroupedEnvironment::Group(group) => group.system_requirements(),
            GroupedEnvironment::Environment(env) => env.system_requirements(),
        }
    }

    pub fn virtual_packages(&self, platform: Platform) -> Vec<GenericVirtualPackage> {
        get_minimal_virtual_packages(platform, &self.system_requirements())
            .into_iter()
            .map(GenericVirtualPackage::from)
            .collect()
    }

    pub fn channels(&self) -> IndexSet<&'p Channel> {
        match self {
            GroupedEnvironment::Group(group) => group.channels(),
            GroupedEnvironment::Environment(env) => env.channels(),
        }
    }

    pub fn has_pypi_dependencies(&self) -> bool {
        match self {
            GroupedEnvironment::Group(group) => group.has_pypi_dependencies(),
            GroupedEnvironment::Environment(env) => env.has_pypi_dependencies(),
        }
    }
}

/// A struct that holds both a ``Vec` of `RepoDataRecord` and a mapping from name to index.
#[derive(Clone, Debug, Default)]
struct RepoDataRecordsByName {
    records: Vec<RepoDataRecord>,
    by_name: HashMap<PackageName, usize>,
}

impl From<Vec<RepoDataRecord>> for RepoDataRecordsByName {
    fn from(records: Vec<RepoDataRecord>) -> Self {
        let by_name = records
            .iter()
            .enumerate()
            .map(|(idx, record)| (record.package_record.name.clone().into(), idx))
            .collect();
        Self { records, by_name }
    }
}

impl RepoDataRecordsByName {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&RepoDataRecord>
    where
        PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<RepoDataRecord> {
        self.records
    }

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = RepoDataRecord>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.package_record.name.clone().into()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if (&records[idx]).package_record.version < record.package_record.version {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }

    /// Constructs a subset of the records in this set that only contain the packages with the given
    /// names and recursively their dependencies.
    pub fn subset(
        &self,
        package_names: impl IntoIterator<Item = PackageName>,
        virtual_packages: &HashSet<PackageName>,
    ) -> Self {
        let mut queue = package_names.into_iter().collect::<Vec<_>>();
        let mut queued_names = queue.iter().cloned().collect::<HashSet<_>>();
        let mut records = Vec::new();
        let mut by_name = HashMap::new();
        while let Some(package) = queue.pop() {
            // Find the record in the superset of records
            let found_package = if virtual_packages.contains(&package) {
                continue;
            } else if let Some(record) = self.by_name(&package) {
                record
            } else {
                continue;
            };

            // Find all the dependencies of the package and add them to the queue
            for dependency in found_package.package_record.depends.iter() {
                let dependency_name = PackageName::new_unchecked(
                    dependency.split_once(' ').unwrap_or((&dependency, "")).0,
                );
                if queued_names.insert(dependency_name.clone().into()) {
                    queue.push(dependency_name.into());
                }
            }

            let idx = records.len();
            by_name.insert(package, idx);
            records.push(found_package.clone());
        }

        Self { records, by_name }
    }
}

type PypiRecord = (PypiPackageData, PypiPackageEnvironmentData);

#[derive(Clone, Debug, Default)]
pub struct PypiRecordsByName {
    records: Vec<PypiRecord>,
    by_name: HashMap<rip::types::PackageName, usize>,
}

impl PypiRecordsByName {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&PypiRecord>
    where
        rip::types::PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<PypiRecord> {
        self.records
    }

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = PypiRecord>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            let Ok(package_name) = rip::types::PackageName::from_str(&record.0.name) else {
                continue;
            };
            match by_name.entry(package_name) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if (&records[idx]).0.version < record.0.version {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }

    /// Constructs a subset of the records in this set that only contain the packages with the given
    /// names and recursively their dependencies.
    pub fn subset(
        &self,
        package_names: impl IntoIterator<Item = rip::types::PackageName>,
        conda_package_identifiers: &HashMap<rip::types::PackageName, PypiPackageIdentifier>,
    ) -> Self {
        let mut queue = package_names.into_iter().collect::<Vec<_>>();
        let mut queued_names = queue.iter().cloned().collect::<HashSet<_>>();
        let mut records = Vec::new();
        let mut by_name = HashMap::new();
        while let Some(package) = queue.pop() {
            // Find the record in the superset of records
            let found_package = if conda_package_identifiers.contains_key(&package) {
                continue;
            } else if let Some(record) = self.by_name(&package) {
                record
            } else {
                continue;
            };

            // Find all the dependencies of the package and add them to the queue
            for dependency in found_package.0.requires_dist.iter() {
                let Ok(dependency_name) = rip::types::PackageName::from_str(&dependency.name)
                else {
                    continue;
                };
                if queued_names.insert(dependency_name.clone()) {
                    queue.push(dependency_name);
                }
            }

            let idx = records.len();
            by_name.insert(package, idx);
            records.push(found_package.clone());
        }

        Self { records, by_name }
    }
}
