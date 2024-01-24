use crate::project::Environment;
use crate::{config, consts, install, install_pypi, lock_file, prefix::Prefix, progress, Project};
use miette::{Context, IntoDiagnostic};

use crate::lock_file::lock_file_satisfies_project;
use crate::project::manifest::SystemRequirements;
use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use itertools::Itertools;
use rattler::install::{PythonInfo, Transaction};
use rattler_conda_types::{Platform, PrefixRecord, RepoDataRecord};
use rattler_lock::{LockFile, PypiPackageData, PypiPackageEnvironmentData};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rip::index::PackageDb;
use rip::resolve::SDistResolution;
use std::error::Error;
use std::fmt::Write;
use std::{io::ErrorKind, path::Path};

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
    project: &Project,
    usage: LockFileUsage,
    mut no_install: bool,
    sparse_repo_data: Option<Vec<SparseRepoData>>,
    sdist_resolution: SDistResolution,
) -> miette::Result<Prefix> {
    let current_platform = Platform::current();

    // Do not install if the platform is not supported
    if !no_install && !project.platforms().contains(&current_platform) {
        tracing::warn!("Not installing dependency on current platform: ({current_platform}) as it is not part of this project's supported platforms.");
        no_install = true;
    }

    // Make sure the project is in a sane state
    sanity_check_project(project)?;

    // Determine which environment to install.
    let environment = project.default_environment();

    // Early out if If there is no lock-file and we are also not allowed to update it.
    if !project.lock_file_path().is_file() && !usage.allows_lock_file_updates() {
        miette::bail!("no lockfile available, can't do a frozen installation.");
    }

    // Start loading the installed packages in the background
    let prefix = Prefix::new(environment.dir())?;
    let installed_packages_future = {
        let prefix = prefix.clone();
        tokio::spawn(async move { prefix.find_installed_packages(None).await })
    };

    // Load the lock-file into memory.
    let lock_file = lock_file::load_lock_file(project).await?;

    // Check if the lock-file is up to date, but only if the current usage allows it.
    let update_lock_file = if usage.should_check_if_out_of_date() {
        match lock_file_satisfies_project(project, &lock_file) {
            Err(err) => {
                // Construct an error message
                let mut report = String::new();
                let mut err: &dyn Error = &err;
                write!(&mut report, "{}", err).unwrap();
                while let Some(source) = err.source() {
                    write!(&mut report, "\nbecause {}", source).unwrap();
                    err = source
                }

                tracing::info!("lock-file is not up to date with the project\nbecause {report}",);

                if !usage.allows_lock_file_updates() {
                    miette::bail!("lock-file not up-to-date with the project");
                }

                true
            }
            Ok(_) => {
                tracing::debug!("the lock-file is up to date with the project.",);
                false
            }
        }
    } else {
        false
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
    let mut updated_repodata_records = None;
    let repodata_records: &_ = if update_lock_file {
        updated_repodata_records.insert(
            lock_file::update_lock_file_conda(
                &environment,
                &locked_repodata_records,
                sparse_repo_data,
            )
            .await?,
        )
    } else {
        &locked_repodata_records
    };

    // Update the prefix with the conda packages. This will also return the python status.
    let python_status = if !no_install {
        let installed_prefix_records = installed_packages_future.await.into_diagnostic()??;
        let empty_vec = Vec::new();
        update_prefix_conda(
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
    let mut updated_pypi_records = None;
    let pypi_records: &_ = if project.has_pypi_dependencies() && update_lock_file {
        let python_path = python_status.location().map(|p| prefix.root().join(p));
        updated_pypi_records.insert(
            lock_file::update_lock_file_for_pypi(
                &environment,
                repodata_records,
                &locked_pypi_records,
                python_path.as_deref(),
                sdist_resolution,
            )
            .await?,
        )
    } else {
        &locked_pypi_records
    };

    if project.has_pypi_dependencies() && !no_install {
        // Then update the pypi packages.
        let empty_repodata_vec = Vec::new();
        let empty_pypi_vec = Vec::new();
        update_prefix_pypi(
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

    // If any of the records have changed we need to update the contents of the lock-file.
    if updated_repodata_records.is_some() || updated_pypi_records.is_some() {
        let mut builder = LockFile::builder();

        let channels = environment
            .channels()
            .into_iter()
            .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
            .collect_vec();
        builder.set_channels(environment.name().as_str(), channels);

        // Add the conda records
        for (platform, records) in updated_repodata_records.unwrap_or(locked_repodata_records) {
            for record in records {
                builder.add_conda_package(environment.name().as_str(), platform, record.into());
            }
        }

        // Add the PyPi records
        for (platform, packages) in updated_pypi_records.unwrap_or(locked_pypi_records) {
            for (pkg_data, pkg_env_data) in packages {
                builder.add_pypi_package(
                    environment.name().as_str(),
                    platform,
                    pkg_data,
                    pkg_env_data,
                );
            }
        }

        // Write to disk
        let lock_file = builder.finish();
        lock_file
            .to_path(&project.lock_file_path())
            .into_diagnostic()
            .context("failed to write updated lock-file to disk")?;
    }

    Ok(prefix)
}

pub async fn get_up_to_date_prefix_from_environment<'p>(
    environment: &'p Environment<'p>,
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

    // Early out if If there is no lock-file and we are also not allowed to update it.
    if !project.lock_file_path().is_file() && !usage.allows_lock_file_updates() {
        miette::bail!("no lockfile available, can't do a frozen installation.");
    }

    // Start loading the installed packages in the background
    let prefix = Prefix::new(environment.dir())?;
    let installed_packages_future = {
        let prefix = prefix.clone();
        tokio::spawn(async move { prefix.find_installed_packages(None).await })
    };

    // Load the lock-file into memory.
    let lock_file = lock_file::load_lock_file(project).await?;

    // Check if the lock-file is up to date, but only if the current usage allows it.
    let update_lock_file = if usage.should_check_if_out_of_date() {
        match lock_file_satisfies_project(project, &lock_file) {
            Err(err) => {
                // Construct an error message
                let mut report = String::new();
                let mut err: &dyn Error = &err;
                write!(&mut report, "{}", err).unwrap();
                while let Some(source) = err.source() {
                    write!(&mut report, "\nbecause {}", source).unwrap();
                    err = source
                }

                tracing::info!("lock-file is not up to date with the project\nbecause {report}",);

                if !usage.allows_lock_file_updates() {
                    miette::bail!("lock-file not up-to-date with the project");
                }

                true
            }
            Ok(_) => {
                tracing::debug!("the lock-file is up to date with the project.",);
                false
            }
        }
    } else {
        false
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
    let mut updated_repodata_records = None;
    let repodata_records: &_ = if update_lock_file {
        updated_repodata_records.insert(
            lock_file::update_lock_file_conda(
                &environment,
                &locked_repodata_records,
                sparse_repo_data,
            )
            .await?,
        )
    } else {
        &locked_repodata_records
    };

    // Update the prefix with the conda packages. This will also return the python status.
    let python_status = if !no_install {
        let installed_prefix_records = installed_packages_future.await.into_diagnostic()??;
        let empty_vec = Vec::new();
        update_prefix_conda(
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
    let mut updated_pypi_records = None;
    let pypi_records: &_ = if project.has_pypi_dependencies() && update_lock_file {
        let python_path = python_status.location().map(|p| prefix.root().join(p));
        updated_pypi_records.insert(
            lock_file::update_lock_file_for_pypi(
                &environment,
                repodata_records,
                &locked_pypi_records,
                python_path.as_deref(),
                sdist_resolution,
            )
            .await?,
        )
    } else {
        &locked_pypi_records
    };

    if project.has_pypi_dependencies() && !no_install {
        // Then update the pypi packages.
        let empty_repodata_vec = Vec::new();
        let empty_pypi_vec = Vec::new();
        update_prefix_pypi(
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

    // If any of the records have changed we need to update the contents of the lock-file.
    if updated_repodata_records.is_some() || updated_pypi_records.is_some() {
        let mut builder = LockFile::builder();

        let channels = environment
            .channels()
            .into_iter()
            .map(|channel| rattler_lock::Channel::from(channel.base_url().to_string()))
            .collect_vec();
        builder.set_channels(environment.name().as_str(), channels);

        // Add the conda records
        for (platform, records) in updated_repodata_records.unwrap_or(locked_repodata_records) {
            for record in records {
                builder.add_conda_package(environment.name().as_str(), platform, record.into());
            }
        }

        // Add the PyPi records
        for (platform, packages) in updated_pypi_records.unwrap_or(locked_pypi_records) {
            for (pkg_data, pkg_env_data) in packages {
                builder.add_pypi_package(
                    environment.name().as_str(),
                    platform,
                    pkg_data,
                    pkg_env_data,
                );
            }
        }

        // Write to disk
        let lock_file = builder.finish();
        lock_file
            .to_path(&project.lock_file_path())
            .into_diagnostic()
            .context("failed to write updated lock-file to disk")?;
    }

    Ok(prefix)
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
