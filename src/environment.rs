use crate::project::Environment;
use crate::{config, consts, install, install_pypi, lock_file, prefix::Prefix, progress, Project};
use miette::{Context, IntoDiagnostic};

use crate::lock_file::{
    load_lock_file, lock_file_satisfies_project, verify_environment_satisfiability,
    verify_platform_satisfiability, LockedCondaPackages, PlatformUnsat,
};
use crate::progress::global_multi_progress;
use crate::project::manifest::{EnvironmentName, SystemRequirements};
use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use crate::repodata::fetch_sparse_repodata_targets;
use crate::utils::BarrierCell;
use async_scoped::{Scope, TokioScope};
use futures::future::Either;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use indicatif::{ProgressBar, ProgressFinish};
use itertools::Itertools;
use rattler::install::{PythonInfo, Transaction};
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, Platform, PrefixRecord, RepoDataRecord,
};
use rattler_lock::{LockFile, PypiPackageData, PypiPackageEnvironmentData};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rip::index::PackageDb;
use rip::resolve::SDistResolution;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::convert::identity;
use std::error::Error;
use std::fmt::Write;
use std::future::{ready, Future};
use std::sync::Arc;
use std::time::Duration;
use std::{io::ErrorKind, path::Path};
use tokio::task::{JoinError, JoinHandle};

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
/// up to date it is updated.
///
/// The `sparse_repo_data` is used when the lock-file is update. We pass it into this function to
/// make sure the data is not loaded twice since the repodata takes up a lot of memory and takes a
/// while to load. If `sparse_repo_data` is `None` it will be downloaded. If the lock-file is not
/// updated, the `sparse_repo_data` is ignored.
pub async fn get_up_to_date_prefix(
    environment: &Environment<'_>,
    usage: LockFileUsage,
    mut no_install: bool,
    sparse_repo_data: Option<Vec<SparseRepoData>>,
    sdist_resolution: SDistResolution,
) -> miette::Result<Prefix> {
    let current_platform = Platform::current();
    let project = environment.project();

    // Do not install if the platform is not supported
    if !no_install && !project.platforms().contains(&current_platform) {
        tracing::warn!("Not installing dependency on current platform: ({current_platform}) as it is not part of this project's supported platforms.");
        no_install = true;
    }

    // Make sure the project is in a sane state
    sanity_check_project(project)?;

    // Determine which environment to install.
    let environment = project.default_environment();

    // Early out if there is no lock-file and we are also not allowed to update it.
    if !project.lock_file_path().is_file() && !usage.allows_lock_file_updates() {
        miette::bail!("no lockfile available, can't do a frozen installation.");
    }

    // Ensure that the lock-file is up-to-date
    let lock_file = ensure_up_to_date_lock_file(project).await?;

    // // Start loading the installed packages in the background
    // let prefix = Prefix::new(environment.dir())?;
    // let installed_packages_future = {
    //     let prefix = prefix.clone();
    //     tokio::spawn(async move { prefix.find_installed_packages(None).await })
    // };
    //
    // // Load the lock-file into memory.
    // let lock_file = lock_file::load_lock_file(project).await?;
    //
    // // Determine which environments/platforms are out of date and should be updated.
    // let mut tasks = Vec::new();
    // let environments = project.environments();
    // for env in environments {
    //     let locked_env = lock_file.environment(env.name().as_str());
    //     match verify_environment_satisfiability(&env, locked_env) {
    //         Ok(_) => {
    //             // Verify each individual platform
    //             for platform in env.platforms() {
    //                 verify_platform_satisfiability(
    //                     &env,
    //                     locked_env.as_ref().expect("must have env"),
    //                     platform,
    //                 )
    //             }
    //         }
    //         Err(unsat) => {
    //             tracing::info!("environment {env} is not satisfiable because {unsat}",);
    //             if !usage.allows_lock_file_updates() {
    //                 miette::bail!("lock-file is not up-to-date with the project",);
    //             }
    //             for platform in env.platforms() {
    //                 tasks.push(Task::SolveConda);
    //             }
    //         }
    //     }
    // }
    //
    // // Check if the lock-file is up to date, but only if the current usage allows it.
    // let update_lock_file = if usage.should_check_if_out_of_date() {
    //     match lock_file_satisfies_project(project, &lock_file) {
    //         Err(err) => {
    //             // Construct an error message
    //             let mut report = String::new();
    //             let mut err: &dyn Error = &err;
    //             write!(&mut report, "{}", err).unwrap();
    //             while let Some(source) = err.source() {
    //                 write!(&mut report, "\nbecause {}", source).unwrap();
    //                 err = source
    //             }
    //
    //             tracing::info!("lock-file is not up to date with the project\nbecause {report}",);
    //
    //             if !usage.allows_lock_file_updates() {
    //                 miette::bail!("lock-file not up-to-date with the project");
    //             }
    //
    //             true
    //         }
    //         Ok(_) => {
    //             tracing::debug!("the lock-file is up to date with the project.",);
    //             false
    //         }
    //     }
    // } else {
    //     false
    // };
    //
    // // Get the environment from the lock-file.
    // let locked_environment = lock_file.environment(environment.name().as_str());
    //
    // // Get all the repodata records from the lock-file
    // let locked_repodata_records = locked_environment
    //     .as_ref()
    //     .map(|env| env.conda_repodata_records())
    //     .transpose()
    //     .into_diagnostic()
    //     .context("failed to parse the contents of the lock-file. Try removing the lock-file and running again")?
    //     .unwrap_or_default();
    //
    // // If the lock-file requires an updates, update the conda records.
    // //
    // // The `updated_repodata_records` fields holds the updated records if the records are updated.
    // //
    // // Depending on whether the lock-filed was updated the `repodata_records` field either points
    // // to the `locked_repodata_records` or to the `updated_repodata_records`.
    // let mut updated_repodata_records = None;
    // let repodata_records: &_ = if update_lock_file {
    //     updated_repodata_records.insert(
    //         lock_file::update_lock_file_conda(
    //             &environment,
    //             &locked_repodata_records,
    //             sparse_repo_data,
    //         )
    //         .await?,
    //     )
    // } else {
    //     &locked_repodata_records
    // };
    //
    // // Update the prefix with the conda packages. This will also return the python status.
    // let python_status = if !no_install {
    //     let installed_prefix_records = installed_packages_future.await.into_diagnostic()??;
    //     let empty_vec = Vec::new();
    //     update_prefix_conda(
    //         &prefix,
    //         project.authenticated_client().clone(),
    //         installed_prefix_records,
    //         repodata_records
    //             .get(&current_platform)
    //             .unwrap_or(&empty_vec),
    //         Platform::current(),
    //     )
    //     .await?
    // } else {
    //     // We don't know and it won't matter because we won't install pypi either
    //     PythonStatus::DoesNotExist
    // };
    //
    // // Get the current pypi dependencies from the lock-file.
    // let locked_pypi_records = locked_environment
    //     .map(|env| env.pypi_packages())
    //     .unwrap_or_default();
    //
    // // If the project has pypi dependencies and we need to update the lock-file lets do so here.
    // //
    // // The `updated_pypi_records` fields holds the updated records if the records are updated.
    // //
    // // Depending on whether the lock-file was updated the `pypi_records` field either points
    // // to the `locked_pypi_records` or to the `updated_pypi_records`.
    // let mut updated_pypi_records = None;
    // let pypi_records: &_ = if project.has_pypi_dependencies() && update_lock_file {
    //     let python_path = python_status.location().map(|p| prefix.root().join(p));
    //     updated_pypi_records.insert(
    //         lock_file::update_lock_file_for_pypi(
    //             &environment,
    //             repodata_records,
    //             &locked_pypi_records,
    //             python_path.as_deref(),
    //             sdist_resolution,
    //         )
    //         .await?,
    //     )
    // } else {
    //     &locked_pypi_records
    // };
    //
    // if project.has_pypi_dependencies() && !no_install {
    //     // Then update the pypi packages.
    //     let empty_repodata_vec = Vec::new();
    //     let empty_pypi_vec = Vec::new();
    //     update_prefix_pypi(
    //         &prefix,
    //         current_platform,
    //         project.pypi_package_db()?,
    //         repodata_records
    //             .get(&current_platform)
    //             .unwrap_or(&empty_repodata_vec),
    //         pypi_records
    //             .get(&current_platform)
    //             .unwrap_or(&empty_pypi_vec),
    //         &python_status,
    //         &project.system_requirements(),
    //         sdist_resolution,
    //     )
    //     .await?;
    // }
    //
    // // If any of the records have changed we need to update the contents of the lock-file.
    // if updated_repodata_records.is_some() || updated_pypi_records.is_some() {
    //     let mut builder = LockFile::builder();
    //
    //     let channels = environment
    //         .channels()
    //         .into_iter()
    //         .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
    //         .collect_vec();
    //     builder.set_channels(environment.name().as_str(), channels);
    //
    //     // Add the conda records
    //     for (platform, records) in updated_repodata_records.unwrap_or(locked_repodata_records) {
    //         for record in records {
    //             builder.add_conda_package(environment.name().as_str(), platform, record.into());
    //         }
    //     }
    //
    //     // Add the PyPi records
    //     for (platform, packages) in updated_pypi_records.unwrap_or(locked_pypi_records) {
    //         for (pkg_data, pkg_env_data) in packages {
    //             builder.add_pypi_package(
    //                 environment.name().as_str(),
    //                 platform,
    //                 pkg_data,
    //                 pkg_env_data,
    //             );
    //         }
    //     }
    //
    //     // Write to disk
    //     let lock_file = builder.finish();
    //     lock_file
    //         .to_path(&project.lock_file_path())
    //         .into_diagnostic()
    //         .context("failed to write updated lock-file to disk")?;
    // }
    //
    // Ok(prefix)

    todo!()
}

#[allow(clippy::too_many_arguments)]
// TODO: refactor args into struct
pub async fn update_prefix_pypi(
    prefix: &Prefix,
    platform: Platform,
    package_db: &PackageDb,
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
        "updating python packages",
        install_pypi::update_python_distributions(
            package_db,
            prefix,
            conda_records,
            pypi_records,
            platform,
            status,
            system_requirements,
            sdist_resolution,
        ),
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
    prefix: &Prefix,
    authenticated_client: AuthenticatedClient,
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
            "updating environment",
            install::execute_transaction(
                &transaction,
                &installed_packages,
                prefix.root().to_path_buf(),
                config::get_cache_dir()?,
                authenticated_client,
            ),
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
#[derive(Default)]
struct LockFileDerivedData {
    /// The lock-file
    pub lock_file: LockFile,

    /// Repodata that was fetched
    pub repo_data: Arc<IndexMap<(Channel, Platform), SparseRepoData>>,
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
                    environment.name().as_str()
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
                    environment.name().as_str()
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
                        environment.name().as_str()
                    );

                        outdated_pypi
                            .entry(environment.clone())
                            .or_default()
                            .insert(platform);
                    }
                    Err(unsat) => {
                        tracing::info!(
                        "the dependencies of environment '{0}' for platform {platform} are out of date because {unsat}",
                        environment.name().as_str()
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

    pub fn is_empty(&self) -> bool {
        self.conda.is_empty() && self.pypi.is_empty()
    }
}

/// Ensures that the lock-file is up-to-date with the project.
///
/// This function will return a [`LockFileDerivedData`] struct that contains the lock-file and any
/// potential derived data that was computed as part of this function. The derived data might be
/// usable by other functions to avoid recomputing the same data.
async fn ensure_up_to_date_lock_file<'p>(
    project: &'p Project,
) -> miette::Result<LockFileDerivedData> {
    let lock_file = load_lock_file(project).await?;
    let current_platform = Platform::current();

    // Check which environments are out of date.
    let outdated = OutdatedEnvironments::from_project_and_lock_file(project, &lock_file);
    if outdated.is_empty() {
        tracing::info!("the lock-file is up-to-date");

        // If no-environment is outdated we can return early.
        return Ok(LockFileDerivedData {
            lock_file,
            ..LockFileDerivedData::default()
        });
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
    let repo_data = Arc::new(
        fetch_sparse_repodata_targets(fetch_targets.into_iter(), project.authenticated_client())
            .await?,
    );

    // TODO(baszalmstra): In the future we could start solving conda environments as soon as we
    //  enough repodata is available.

    // Extract the current conda records from the lock-file
    // TODO: Should we parallelize this? Measure please.
    let mut locked_conda_repodata_records = project
        .environments()
        .into_iter()
        .flat_map(|env| {
            lock_file
                .environment(env.name().as_str())
                .into_iter()
                .map(move |locked_env| {
                    locked_env
                        .conda_repodata_records()
                        .map(|records| (env.clone(), records))
                })
        })
        .collect::<Result<HashMap<_, _>, _>>()
        .into_diagnostic()?;

    // Construct all progress bars before spawning anything because that causes the progress bar to
    // skip a line or something.
    let mut environment_progress_bars = HashMap::new();
    let mut target_progress_bars = HashMap::new();
    for (environment, platforms) in outdated.conda.iter() {
        let mut environment_pb = EnvironmentProgressBar::new(environment.name());
        for platform in platforms {
            target_progress_bars.insert(
                (environment.clone(), *platform),
                environment_pb.add_platform(*platform),
            );
        }
        environment_progress_bars.insert(environment.clone(), environment_pb);
    }

    let mut pending_futures = FuturesUnordered::new();
    let mut updated_conda_records: HashMap<_, HashMap<_, _>> = HashMap::new();

    // Start solving all the conda environments.
    for (environment, platforms) in outdated.conda {
        let updated_conda_records = updated_conda_records
            .entry(environment.clone())
            .or_default();

        for platform in platforms {
            // Get the progress bar for this target
            let pb = target_progress_bars
                .get(&(environment.clone(), platform))
                .expect("progress bar must exist")
                .clone();

            // Extract the records from the existing lock file
            let existing_records = locked_conda_repodata_records
                .get_mut(&environment)
                .and_then(|records| records.remove(&platform))
                .unwrap_or_default();

            // Spawn a task to solve the environment.
            let conda_solve_task = spawn_solve_conda_environment_task(
                &environment,
                existing_records,
                &repo_data,
                platform,
                pb,
            );

            pending_futures.push(conda_solve_task);
            updated_conda_records.insert(platform, BarrierCell::new());
        }
    }

    // Start creating/updating pypi build environments
    for environment in outdated.pypi.keys() {
        // Construct a future that will resolve when we have the repodata available for the current
        // platform for this environment.
        let records_future = updated_conda_records
            .get(environment)
            .and_then(|records| records.get(&current_platform))
            .map(|records| Either::Left(records.wait()))
            .or_else(|| {
                locked_conda_repodata_records
                    .get(environment)
                    .and_then(|records| records.get(&current_platform))
                    .map(|records| Either::Right(ready(records)))
            })
            .expect("conda records should be available now or in the future");

        // Spawn a task to instantiate the environment
        let pypi_env_task = spawn_create_prefix_task(environment, records_future);
    }

    // Iterate over all pending futures and collect them one by one
    while let Some(result) = pending_futures.next().await {
        let result = match result.map_err(JoinError::try_into_panic) {
            Ok(Ok(result)) => result,
            Ok(Err(err)) => return Err(err),
            Err(Err(_)) => miette::bail!("cancelled"),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
        };

        match result {
            TaskResult::CondaSolved(environment, platform, records) => {
                let environment = project
                    .environment(&environment)
                    .expect("environment should exist");

                updated_conda_records
                    .entry(environment.clone())
                    .or_default()
                    .entry(platform)
                    .or_default()
                    .set(records)
                    .expect("records should not be solved twice");
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
            let records = updated_conda_records
                .get_mut(&environment)
                .and_then(|records| records.remove(&platform))
                .map(|records| records.into_inner().expect("records should be solved"))
                .or_else(|| {
                    locked_conda_repodata_records
                        .get_mut(&environment)
                        .and_then(|records| records.remove(&platform))
                })
                .expect("the records should either have been updated or they should already exist");

            for record in records {
                builder.add_conda_package(environment.name().as_str(), platform, record.into());
            }
        }
    }

    // Drop progress bars to clear all of them
    drop(environment_progress_bars);

    // Store the lock file
    let lock_file = builder.finish();
    lock_file
        .to_path(&project.lock_file_path())
        .into_diagnostic()
        .context("failed to write lock-file to disk")?;

    Ok(LockFileDerivedData {
        lock_file,
        repo_data,
    })
}

struct EnvironmentProgressBar {
    top_level: ProgressBar,
    children: Vec<ProgressBar>,
}

impl EnvironmentProgressBar {
    pub fn new(name: &EnvironmentName) -> Self {
        let top_level_progress = global_multi_progress().add(ProgressBar::new(10));
        top_level_progress.set_style(progress::long_running_progress_style());
        top_level_progress.set_message(format!("Solving environment '{}'", name.as_str()));
        top_level_progress.enable_steady_tick(Duration::from_millis(100));

        Self {
            top_level: top_level_progress,
            children: Vec::new(),
        }
    }

    pub fn add_platform(&mut self, platform: Platform) -> SolveProgressBar {
        let last_pb = self.children.last().unwrap_or(&self.top_level);
        let pb = global_multi_progress().insert_after(last_pb, ProgressBar::new(10));
        self.children.push(pb.clone());

        SolveProgressBar::new(pb, platform)
    }
}

impl Drop for EnvironmentProgressBar {
    fn drop(&mut self) {
        for pb in self.children.iter().rev() {
            pb.finish_and_clear();
        }
        self.top_level.finish_and_clear();
    }
}

#[derive(Clone)]
struct SolveProgressBar {
    pb: ProgressBar,
    platform: Platform,
}

impl SolveProgressBar {
    pub fn new(pb: ProgressBar, platform: Platform) -> Self {
        pb.set_style(
            indicatif::ProgressStyle::with_template(
                &format!("    {:<9} ..", platform.to_string(),),
            )
            .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));

        Self { pb, platform }
    }

    pub fn start(&self) {
        self.pb.reset_elapsed();
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {{spinner:.dim}} {:<9} [{{elapsed_precise}}] {{msg:.dim}}",
                self.platform.to_string(),
            ))
            .unwrap(),
        );
    }

    pub fn set_message(&self, message: impl Into<Cow<'static, str>>) {
        self.pb.set_message(message);
    }

    pub fn finish(&self) {
        self.pb.set_style(
            indicatif::ProgressStyle::with_template(&format!(
                "  {} {:<9} [{{elapsed_precise}}]",
                console::style(console::Emoji("✔", "↳")).green(),
                self.platform.to_string(),
            ))
            .unwrap(),
        );
        self.pb.finish();
    }
}

fn spawn_solve_conda_environment_task(
    environment: &Environment<'_>,
    existing_repodata_records: Vec<RepoDataRecord>,
    sparse_repo_data: &Arc<IndexMap<(Channel, Platform), SparseRepoData>>,
    platform: Platform,
    pb: SolveProgressBar,
) -> JoinHandle<miette::Result<TaskResult>> {
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

    /// Whether there are pypi dependencies, and we should fetch purls.
    let has_pypi_dependencies = environment.has_pypi_dependencies();

    tokio::spawn((move || async move {
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

        tokio::time::sleep(Duration::new(platform.as_str().len() as u64, 0)).await;

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
    })())
}

pub enum TaskResult {
    CondaSolved(EnvironmentName, Platform, Vec<RepoDataRecord>),
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
    .map_or_else(|e| Err(e), identity)
    .with_context(|| {
        format!(
            "failed to load repodata records for platform '{}'",
            platform.as_str()
        )
    })
}

fn spawn_create_prefix_task(
    environment: &Environment<'_>,
    records_future: impl Future<Output = &Vec<RepoDataRecord>>,
) -> JoinHandle<miette::Result<TaskResult>> {

}
