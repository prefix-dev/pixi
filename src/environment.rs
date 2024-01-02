use crate::{
    consts, default_authenticated_client, install, install_pypi, lock_file, prefix::Prefix,
    progress, virtual_packages::verify_current_platform_has_required_virtual_packages, Project,
};
use miette::{Context, IntoDiagnostic, LabeledSpan};

use crate::lock_file::lock_file_satisfies_project;
use rattler::install::Transaction;
use rattler_conda_types::{Platform, PrefixRecord};
use rattler_lock::CondaLock;
use rip::index::PackageDb;
use std::path::PathBuf;
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
pub fn sanity_check_project(project: &Project) -> miette::Result<()> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.pixi_dir().join(consts::PREFIX_FILE_NAME).as_path())?;

    // Make sure the project supports the current platform
    let platform = Platform::current();
    if !project.platforms().contains(&platform) {
        let span = project.manifest.parsed.project.platforms.span();
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
    }

    // Make sure the system requirements are met
    verify_current_platform_has_required_virtual_packages(project)?;

    Ok(())
}

/// Specifies how the lock-file should be updated.
#[derive(Debug, Default)]
pub enum LockFileUsage {
    /// Update the lock-file if it is out of date.
    #[default]
    Update,
    /// Don't update the lock-file, but do check if it is out of date
    Locked,
    /// Don't update the lock-file and don't check if it is out of date
    Frozen,
}

/// Returns the prefix associated with the given environment. If the prefix doesn't exist or is not
/// up to date it is updated.
pub async fn get_up_to_date_prefix(
    project: &Project,
    usage: LockFileUsage,
    no_install: bool,
) -> miette::Result<Prefix> {
    // Make sure the project is in a sane state
    sanity_check_project(project)?;

    // Start loading the installed packages in the background
    let prefix = Prefix::new(project.environment_dir())?;
    let installed_packages_future = {
        let prefix = prefix.clone();
        tokio::spawn(async move { prefix.find_installed_packages(None).await })
    };

    // Update the lock-file if it is out of date.
    if matches!(usage, LockFileUsage::Frozen) && !project.lock_file_path().is_file() {
        miette::bail!("No lockfile available, can't do a frozen installation.");
    }

    let mut lock_file = lock_file::load_lock_file(project).await?;
    let up_to_date = lock_file_satisfies_project(project, &lock_file)?;
    if matches!(usage, LockFileUsage::Locked) && !up_to_date {
        miette::bail!("Lockfile not up-to-date with the project");
    }
    let update_lock = matches!(usage, LockFileUsage::Update) && !up_to_date;

    // First lock and install the conda environment
    // After which we should have a usable prefix to use for pypi resolution.
    if update_lock {
        lock_file = lock_file::update_lock_file_conda(project, lock_file, None).await?;
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
        if update_lock {
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
    Changed((u32, u32, u32), PathBuf),
    Unchanged((u32, u32, u32), PathBuf),
    DoesNotExist,
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
    let python_changed = {
        if let Some(python_info) = transaction.current_python_info.as_ref() {
            let python_version = (
                python_info.short_version.0 as u32,
                python_info.short_version.1 as u32,
                0,
            );

            if Some(python_info.short_version)
                == transaction.python_info.as_ref().map(|p| p.short_version)
            {
                PythonStatus::Unchanged(python_version, python_info.path.clone())
            } else {
                // Determine the current python distributions in its install locations
                PythonStatus::Changed(python_version, python_info.path.clone())
            }
        } else {
            PythonStatus::DoesNotExist
        }
    };
    Ok(python_changed)
}
