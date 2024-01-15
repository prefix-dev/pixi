use crate::{
    consts, default_authenticated_client, install, install_pypi, lock_file, prefix::Prefix,
    progress, Project,
};
use miette::{Context, IntoDiagnostic, LabeledSpan};

use crate::lock_file::lock_file_satisfies_project;
use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use rattler::install::{PythonInfo, Transaction};
use rattler_conda_types::{Platform, PrefixRecord, RepoDataRecord};
use rattler_lock::CondaLock;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rip::index::PackageDb;
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

/// Runs a number of different checks to make sure the project is in a sane state:
///     1. It verifies that the prefix location is unchanged.
///     2. It verifies that the project supports the current platform.
///     3. It verifies that the system requirements are met.
pub fn sanity_check_project(project: &Project, no_install: bool) -> miette::Result<()> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.pixi_dir().join(consts::PREFIX_FILE_NAME).as_path())?;

    // Make sure the project supports the current platform
    let platform = Platform::current();
    if !project.platforms().contains(&platform) {
        let span = project.manifest.parsed.project.platforms.span();
        if no_install {
            tracing::warn!("Adding dependency for unsupported platform ({platform}).")
        } else {
            return Err(miette::miette!(
                help = format!(
                    "The project needs to be configured to support your platform ({platform})."
                ),
                labels = vec![LabeledSpan::at(
                    span.unwrap_or_default(),
                    format!("add '{platform}' here"),
                )],
                "the project is not configured for your current platform"
            )
            .with_source_code(project.manifest_named_source()));
        };
    }

    // Make sure the system requirements are met
    verify_current_platform_has_required_virtual_packages(&project.default_environment())?;

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
    no_install: bool,
    sparse_repo_data: Option<Vec<SparseRepoData>>,
) -> miette::Result<Prefix> {
    // Make sure the project is in a sane state
    sanity_check_project(project, no_install)?;

    // Start loading the installed packages in the background
    let prefix = Prefix::new(project.environment_dir())?;
    let installed_packages_future = {
        let prefix = prefix.clone();
        tokio::spawn(async move { prefix.find_installed_packages(None).await })
    };

    // If there is no lock-file and we are also not allowed to update it, we can bail immediately.
    if !project.lock_file_path().is_file() && !usage.allows_lock_file_updates() {
        miette::bail!("no lockfile available, can't do a frozen installation.");
    }

    // Load the lock-file into memory.
    let mut lock_file = lock_file::load_lock_file(project).await?;

    // Check if the lock-file is up to date, but only if the current usage allows it.
    let update_lock_file = if usage.should_check_if_out_of_date()
        && !lock_file_satisfies_project(project, &lock_file)?
    {
        if !usage.allows_lock_file_updates() {
            miette::bail!("lockfile not up-to-date with the project");
        }
        true
    } else {
        false
    };

    // First lock and install the conda environment
    // After which we should have a usable prefix to use for pypi resolution.
    if update_lock_file {
        lock_file = lock_file::update_lock_file_conda(project, lock_file, sparse_repo_data).await?;
    }

    let python_status = if !no_install {
        update_prefix_conda(
            &prefix,
            installed_packages_future.await.into_diagnostic()??,
            &lock_file,
            Platform::current(),
        )
        .await?
    } else {
        // We don't know and it won't matter because we won't install pypi either
        PythonStatus::DoesNotExist
    };

    if project.has_pypi_dependencies() {
        if update_lock_file {
            lock_file = lock_file::update_lock_file_for_pypi(project, lock_file).await?;
        }

        if !no_install {
            // Then update the pypi packages.
            update_prefix_pypi(
                &prefix,
                Platform::current(),
                project.pypi_package_db()?,
                &lock_file,
                &python_status,
            )
            .await?;
        }
    }

    Ok(prefix)
}

pub async fn update_prefix_pypi(
    prefix: &Prefix,
    platform: Platform,
    package_db: &PackageDb,
    lock_file: &CondaLock,
    status: &PythonStatus,
) -> miette::Result<()> {
    // Remove python packages from a previous python distribution if the python version changed.
    install_pypi::remove_old_python_distributions(prefix, platform, status)?;

    // Install and/or remove python packages
    progress::await_in_progress(
        "updating python packages",
        install_pypi::update_python_distributions(package_db, prefix, lock_file, platform, status),
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
}

/// Updates the environment to contain the packages from the specified lock-file
pub async fn update_prefix_conda(
    prefix: &Prefix,
    installed_packages: Vec<PrefixRecord>,
    lock_file: &CondaLock,
    platform: Platform,
) -> miette::Result<PythonStatus> {
    // Construct a transaction to bring the environment up to date with the lock-file content
    let desired_conda_packages = lock_file
        .get_conda_packages_by_platform(platform)
        .into_diagnostic()?;
    let transaction =
        Transaction::from_current_and_desired(installed_packages, desired_conda_packages, platform)
            .into_diagnostic()?;

    // Execute the transaction if there is work to do
    if !transaction.operations.is_empty() {
        // Execute the operations that are returned by the solver.
        progress::await_in_progress(
            "updating environment",
            install::execute_transaction(
                &transaction,
                prefix.root().to_path_buf(),
                rattler::default_cache_dir()
                    .map_err(|_| miette::miette!("could not determine default cache directory"))?,
                default_authenticated_client(),
            ),
        )
        .await?;
    }

    // Mark the location of the prefix
    create_prefix_location_file(
        &prefix
            .root()
            .parent()
            .map(|p| p.join(consts::PREFIX_FILE_NAME))
            .ok_or_else(|| miette::miette!("we should be able to create a prefix file name."))?,
    )
    .with_context(|| "failed to create prefix location file.".to_string())?;

    // Determine if the python version changed.
    Ok(PythonStatus::from_transaction(&transaction))
}
