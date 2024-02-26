use crate::lock_file::UvResolutionContext;
use crate::project::grouped_environment::GroupedEnvironmentName;
use crate::{
    consts, install, install_pypi,
    lock_file::UpdateLockFileOptions,
    prefix::Prefix,
    progress,
    project::{
        grouped_environment::GroupedEnvironment,
        manifest::{EnvironmentName, SystemRequirements},
        virtual_packages::verify_current_platform_has_required_virtual_packages,
        Environment,
    },
    Project,
};
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use rattler::{
    install::{PythonInfo, Transaction},
    package_cache::PackageCache,
};
use rattler_conda_types::{Channel, Platform, PrefixRecord, RepoDataRecord};
use rattler_lock::{PypiPackageData, PypiPackageEnvironmentData};
use rattler_repodata_gateway::sparse::SparseRepoData;
use reqwest_middleware::ClientWithMiddleware;
use std::{collections::HashMap, io::ErrorKind, path::Path, sync::Arc};

/// Verify the location of the prefix folder is not changed so the applied prefix path is still valid.
/// Errors when there is a file system error or the path does not align with the defined prefix.
/// Returns false when the file is not present.
pub fn verify_prefix_location_unchanged(prefix_file: &Path) -> miette::Result<()> {
    match std::fs::read_to_string(prefix_file) {
        // Not found is fine as it can be new or backwards compatible.
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        // Scream the error if we don't know it.
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
            ..UpdateLockFileOptions::default()
        })
        .await?;

    // Get the locked environment from the lock-file.
    if no_install {
        Ok(Prefix::new(environment.dir()))
    } else {
        lock_file.prefix(environment).await
    }
}

#[allow(clippy::too_many_arguments)]
// TODO: refactor args into struct
pub async fn update_prefix_pypi(
    environment_name: &EnvironmentName,
    prefix: &Prefix,
    _platform: Platform,
    conda_records: &[RepoDataRecord],
    pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
    status: &PythonStatus,
    system_requirements: &SystemRequirements,
    uv_context: UvResolutionContext,
) -> miette::Result<()> {
    // Remove python packages from a previous python distribution if the python version changed.
    // install_pypi::remove_old_python_distributions(prefix, platform, status)?;

    // Install and/or remove python packages
    progress::await_in_progress(
        format!(
            "updating pypi package in '{}'",
            environment_name.fancy_display()
        ),
        |_| {
            install_pypi::update_python_distributions(
                prefix,
                conda_records,
                pypi_records,
                status,
                system_requirements,
                uv_context,
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

pub type PerEnvironment<'p, T> = HashMap<Environment<'p>, T>;
pub type PerGroup<'p, T> = HashMap<GroupedEnvironment<'p>, T>;
pub type PerEnvironmentAndPlatform<'p, T> = PerEnvironment<'p, HashMap<Platform, T>>;
pub type PerGroupAndPlatform<'p, T> = PerGroup<'p, HashMap<Platform, T>>;
