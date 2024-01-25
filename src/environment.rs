use crate::project::Environment;
use crate::{config, consts, install, install_pypi, lock_file, prefix::Prefix, progress, Project};
use miette::{Context, IntoDiagnostic};

use crate::lock_file::verify_environment_satisfiability;
use crate::project::manifest::SystemRequirements;
use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use crate::repodata::fetch_sparse_repodata_targets;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use rattler::install::{PythonInfo, Transaction};
use rattler_conda_types::{Channel, Platform, PrefixRecord, RepoDataRecord};
use rattler_lock::{LockFile, PypiPackageData, PypiPackageEnvironmentData};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rip::index::PackageDb;
use rip::resolve::SDistResolution;
use std::{collections::HashMap, error::Error, fmt::Write, io::ErrorKind, path::Path, sync::Arc};

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
pub async fn get_up_to_date_prefix<'p>(
    prefix_env: &'p Environment<'p>,
    usage: LockFileUsage,
    mut no_install: bool,
    sparse_repo_data: Option<IndexMap<(Channel, Platform), SparseRepoData>>,
    sdist_resolution: SDistResolution,
) -> miette::Result<Prefix> {
    let current_platform = Platform::current();
    let project = prefix_env.project();

    // Do not install if the platform is not supported
    if !no_install && !project.platforms().contains(&current_platform) {
        tracing::warn!("Not installing dependency on current platform: ({current_platform}) as it is not part of this project's supported platforms.");
        no_install = true;
    }

    // Make sure the project is in a sane state
    sanity_check_project(project)?;

    // Early out if there is no lock-file and we are also not allowed to update it.
    if !project.lock_file_path().is_file() && !usage.allows_lock_file_updates() {
        miette::bail!("no lockfile available, can't do a frozen installation.");
    }

    // Load the lock-file into memory.
    let lock_file = lock_file::load_lock_file(project).await?;

    let out_of_date_environments = if usage.should_check_if_out_of_date() {
        let mut out_of_date_environments = IndexSet::new();
        for environment in project.environments() {
            // Determine if we need to update this environment
            match verify_environment_satisfiability(
                &environment,
                lock_file.environment(environment.name().as_str()),
            ) {
                Ok(_) => {}
                Err(err) => {
                    // Construct an error message
                    let mut report = String::new();
                    let mut err: &dyn Error = &err;
                    write!(&mut report, "{}", err).unwrap();
                    while let Some(source) = err.source() {
                        write!(&mut report, ", because {}", source).unwrap();
                        err = source
                    }

                    tracing::info!("environment '{}' in the lock-file is not up to date with the project, because {report}", environment.name());

                    out_of_date_environments.insert(environment);
                }
            }
        }

        out_of_date_environments
    } else {
        IndexSet::default()
    };

    // If there are out of date environments but we are not allowed to update the lock-file, error out.
    if !out_of_date_environments.is_empty() && !usage.allows_lock_file_updates() {
        miette::bail!("lock-file not up-to-date with the project");
    }

    // Download all the required repodata
    let targets_to_fetch = out_of_date_environments
        .iter()
        .flat_map(|env| {
            let mut platforms = env.platforms();
            platforms.insert(Platform::NoArch);
            env.channels()
                .into_iter()
                .cloned()
                .cartesian_product(platforms.into_iter().collect_vec())
        })
        .filter(|target| {
            sparse_repo_data
                .as_ref()
                .map(|p| !p.contains_key(target))
                .unwrap_or(true)
        })
        .collect::<IndexSet<_>>();
    let mut fetched_repo_data =
        fetch_sparse_repodata_targets(targets_to_fetch, project.authenticated_client()).await?;
    fetched_repo_data.extend(sparse_repo_data.into_iter().flatten());
    let fetched_repo_data = Arc::new(fetched_repo_data);

    let mut updated_conda_records: HashMap<_, HashMap<_, _>> = HashMap::new();
    let mut updated_pypi_records: HashMap<_, HashMap<_, _>> = HashMap::new();
    let mut old_repodata_records = HashMap::new();
    let mut old_pypi_records = HashMap::new();

    // Iterate over all environments in the project
    for environment in project.environments() {
        let is_wanted_environment = environment == *prefix_env;
        let is_out_of_date_environment = out_of_date_environments.contains(&environment);

        // If this environment is not out of date and also not the environment we are installing, we
        // can skip it.
        if !is_out_of_date_environment && !is_wanted_environment {
            continue;
        }

        // Start loading the installed packages in the background
        let prefix = Prefix::new(environment.dir())?;
        let installed_packages_future = {
            let prefix = prefix.clone();
            tokio::spawn(async move { prefix.find_installed_packages(None).await })
        };

        // Get the environment from the lock-file.
        let locked_environment = lock_file.environment(environment.name().as_str());

        // Get all the repodata records from the lock-file
        let locked_repodata_records = locked_environment
            .as_ref()
            .map(|env| env.conda_repodata_records())
            .transpose()
            .into_diagnostic()
            .context("failed to parse the contents of the lock-file. Try removing the lock-file and running again")?
            .unwrap_or_default();

        // If the lock-file requires an updates, update the conda records.
        //
        // The `updated_repodata_records` fields holds the updated records if the records are updated.
        //
        // Depending on whether the lock-filed was updated the `repodata_records` field either points
        // to the `locked_repodata_records` or to the `updated_repodata_records`.
        let repodata_records: &_ = if is_out_of_date_environment {
            let records = lock_file::update_lock_file_conda(
                &environment,
                &locked_repodata_records,
                &fetched_repo_data,
            )
            .await?;

            updated_conda_records
                .entry(environment.clone())
                .or_insert(records)
        } else {
            &locked_repodata_records
        };

        let should_update_prefix = is_wanted_environment
            || (is_out_of_date_environment && environment.has_pypi_dependencies());

        // Update the prefix with the conda packages. This will also return the python status.
        let python_status = if should_update_prefix && !no_install {
            let installed_prefix_records = installed_packages_future.await.into_diagnostic()??;
            let empty_vec = Vec::new();
            update_prefix_conda(
                environment.name().as_str(),
                &prefix,
                project.authenticated_client().clone(),
                installed_prefix_records,
                repodata_records
                    .get(&current_platform)
                    .unwrap_or(&empty_vec),
                Platform::current(),
            )
            .await?
        } else {
            // We don't know and it won't matter because we won't install pypi either
            PythonStatus::DoesNotExist
        };

        // If there are no pypi dependencies, we don't need to do anything else.
        if !environment.has_pypi_dependencies() {
            continue;
        }

        // Get the current pypi dependencies from the lock-file.
        let locked_pypi_records = locked_environment
            .map(|env| env.pypi_packages())
            .unwrap_or_default();

        // If the project has pypi dependencies and we need to update the lock-file lets do so here.
        //
        // The `updated_pypi_records` fields holds the updated records if the records are updated.
        //
        // Depending on whether the lock-file was updated the `pypi_records` field either points
        // to the `locked_pypi_records` or to the `updated_pypi_records`.
        let pypi_records: &_ = if is_out_of_date_environment {
            let python_path = python_status.location().map(|p| prefix.root().join(p));
            let records = lock_file::update_lock_file_for_pypi(
                &environment,
                repodata_records,
                &locked_pypi_records,
                python_path.as_deref(),
                sdist_resolution,
            )
            .await?;

            updated_pypi_records
                .entry(environment.clone())
                .or_insert(records)
        } else {
            &locked_pypi_records
        };

        // If there are
        if is_wanted_environment && pypi_records.get(&current_platform).is_some() && !no_install {
            // Then update the pypi packages.
            let empty_repodata_vec = Vec::new();
            let empty_pypi_vec = Vec::new();
            update_prefix_pypi(
                environment.name().as_str(),
                &prefix,
                current_platform,
                project.pypi_package_db()?,
                repodata_records
                    .get(&current_platform)
                    .unwrap_or(&empty_repodata_vec),
                pypi_records
                    .get(&current_platform)
                    .unwrap_or(&empty_pypi_vec),
                &python_status,
                &project.system_requirements(),
                sdist_resolution,
            )
            .await?;
        }

        old_repodata_records.insert(environment.clone(), locked_repodata_records);
        old_pypi_records.insert(environment, locked_pypi_records);
    }

    // If any of the records have changed we need to update the contents of the lock-file.
    if !updated_conda_records.is_empty() || !updated_pypi_records.is_empty() {
        let mut builder = LockFile::builder();

        for environment in project.environments() {
            let channels = environment
                .channels()
                .into_iter()
                .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
                .collect_vec();
            builder.set_channels(environment.name().as_str(), channels);

            let mut loaded_repodata_records = old_repodata_records
                .remove(&environment)
                .unwrap_or_default();
            let mut loaded_pypi_records = old_pypi_records.remove(&environment).unwrap_or_default();

            let mut updated_repodata_records = updated_conda_records
                .remove(&environment)
                .unwrap_or_default();
            let mut updated_pypi_records = updated_pypi_records
                .remove(&environment)
                .unwrap_or_default();

            let locked_environment = lock_file.environment(environment.name().as_str());

            for platform in environment.platforms() {
                let repodata_records = if let Some(records) = updated_repodata_records
                    .remove(&platform)
                    .or_else(|| loaded_repodata_records.remove(&platform))
                {
                    Some(records)
                } else if let Some(locked_environment) = locked_environment.as_ref() {
                    locked_environment
                        .conda_repodata_records_for_platform(platform)
                        .into_diagnostic()
                        .context("failed to load conda repodata records from the lock-file")?
                } else {
                    None
                };
                for record in repodata_records.into_iter().flatten() {
                    builder.add_conda_package(environment.name().as_str(), platform, record.into());
                }

                let pypi_records = if let Some(records) = updated_pypi_records
                    .remove(&platform)
                    .or_else(|| loaded_pypi_records.remove(&platform))
                {
                    Some(records)
                } else if let Some(locked_environment) = locked_environment.as_ref() {
                    locked_environment.pypi_packages_for_platform(platform)
                } else {
                    None
                };
                for (pkg_data, env_data) in pypi_records.into_iter().flatten() {
                    builder.add_pypi_package(
                        environment.name().as_str(),
                        platform,
                        pkg_data,
                        env_data,
                    );
                }
            }
        }

        // Write to disk
        let lock_file = builder.finish();
        lock_file
            .to_path(&project.lock_file_path())
            .into_diagnostic()
            .context("failed to write updated lock-file to disk")?;
    }

    Prefix::new(prefix_env.dir())
}

#[allow(clippy::too_many_arguments)]
// TODO: refactor args into struct
pub async fn update_prefix_pypi(
    name: &str,
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
        format!("updating pypi packages in '{0}' environment", name),
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
    name: &str,
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
            format!("updating packages in '{0}' environment", name),
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
