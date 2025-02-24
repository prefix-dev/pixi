pub(crate) mod conda_metadata;
mod conda_prefix;
mod pypi_prefix;
mod python_status;
mod reporters;

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use dialoguer::theme::ColorfulTheme;
use miette::{Context, IntoDiagnostic};
use pixi_consts::consts;
use pixi_git::credentials::store_credentials_from_url;
use pixi_manifest::{FeaturesExt, PyPiRequirement};
use pixi_progress::await_in_progress;
use pixi_spec::{GitSpec, PixiSpec};
use rattler_conda_types::Platform;
use rattler_lock::LockedPackageRef;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

use crate::{
    lock_file::{LockFileDerivedData, UpdateLockFileOptions, UpdateMode},
    prefix::Prefix,
    rlimit::try_increase_rlimit_to_sensible,
    workspace::{grouped_environment::GroupedEnvironment, Environment, HasWorkspaceRef},
    Workspace,
};

pub use conda_prefix::{CondaPrefixUpdated, CondaPrefixUpdater, CondaPrefixUpdaterBuilder};
pub use pypi_prefix::update_prefix_pypi;
pub use python_status::PythonStatus;

/// Verify the location of the prefix folder is not changed so the applied
/// prefix path is still valid. Errors when there is a file system error or the
/// path does not align with the defined prefix. Returns false when the file is
/// not present.
pub async fn verify_prefix_location_unchanged(environment_dir: &Path) -> miette::Result<()> {
    let prefix_file = environment_dir
        .join(consts::CONDA_META_DIR)
        .join(consts::PREFIX_FILE_NAME);

    tracing::info!(
        "verifying prefix location is unchanged, with prefix file: {}",
        prefix_file.display()
    );

    match fs_err::read_to_string(prefix_file.clone()) {
        // Not found is fine as it can be new or backwards compatible.
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        // Scream the error if we don't know it.
        Err(e) => {
            tracing::error!("failed to read prefix file: {}", prefix_file.display());
            Err(e).into_diagnostic()
        }
        // Check if the path in the file aligns with the current path.
        Ok(p) if prefix_file.starts_with(&p) => Ok(()),
        Ok(p) => {
            let path = Path::new(&p);
            prefix_location_changed(environment_dir, path.parent().unwrap_or(path)).await
        }
    }
}

/// Called when the prefix has moved to a new location.
///
/// Allows interactive users to delete the location and continue.
async fn prefix_location_changed(
    environment_dir: &Path,
    previous_dir: &Path,
) -> miette::Result<()> {
    let theme = ColorfulTheme {
        active_item_style: console::Style::new().for_stderr().magenta(),
        ..ColorfulTheme::default()
    };

    let user_value = dialoguer::Confirm::with_theme(&theme)
        .with_prompt(format!(
            "The environment directory seems have to moved! Environments are non-relocatable, moving them can cause issues.\n\n\t{} -> {}\n\nThis can be fixed by reinstall the environment from the lock-file in the new location.\n\nDo you want to automatically recreate the environment?",
            previous_dir.display(),
            environment_dir.display()
        ))
        .report(false)
        .default(true)
        .interact_opt()
        .map_or(None, std::convert::identity);
    if user_value == Some(true) {
        await_in_progress("removing old environment", |_| {
            tokio::fs::remove_dir_all(environment_dir)
        })
        .await
        .into_diagnostic()
        .context("failed to remove old environment directory")?;
        Ok(())
    } else {
        Err(miette::diagnostic!(
            help = "Remove the environment directory, pixi will recreate it on the next run.",
            "The environment directory has moved from `{}` to `{}`. Environments are non-relocatable, moving them can cause issues.", previous_dir.display(), environment_dir.display()
        )
        .into())
    }
}

#[derive(Debug, Hash, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedEnvironmentHash(String);
impl LockedEnvironmentHash {
    pub(crate) fn from_environment(
        environment: rattler_lock::Environment,
        platform: Platform,
    ) -> Self {
        let mut hasher = Xxh3::new();

        if let Some(packages) = environment.packages(platform) {
            for package in packages {
                // Always has the url or path
                package.location().to_owned().to_string().hash(&mut hasher);

                match package {
                    // A select set of fields are used to hash the package
                    LockedPackageRef::Conda(pack) => {
                        if let Some(sha) = pack.record().sha256 {
                            sha.hash(&mut hasher);
                        } else if let Some(md5) = pack.record().md5 {
                            md5.hash(&mut hasher);
                        }
                    }
                    LockedPackageRef::Pypi(pack, env) => {
                        pack.editable.hash(&mut hasher);
                        env.extras.hash(&mut hasher);
                    }
                }
            }
        }

        LockedEnvironmentHash(format!("{:x}", hasher.finish()))
    }
}

/// Information about the environment that was used to create the environment.
#[derive(Serialize, Deserialize)]
pub(crate) struct EnvironmentFile {
    /// The path to the manifest file that was used to create the environment.
    pub(crate) manifest_path: PathBuf,
    /// The name of the environment.
    pub(crate) environment_name: String,
    /// The version of the pixi that was used to create the environment.
    pub(crate) pixi_version: String,
    /// The hash of the lock file that was used to create the environment.
    pub(crate) environment_lock_file_hash: LockedEnvironmentHash,
}

/// The path to the environment file in the `conda-meta` directory of the
/// environment.
fn environment_file_path(environment_dir: &Path) -> PathBuf {
    environment_dir
        .join(consts::CONDA_META_DIR)
        .join(consts::ENVIRONMENT_FILE_NAME)
}
/// Write information about the environment to a file in the environment
/// directory. Used by the prefix updating to validate if it needs to be
/// updated.
pub(crate) fn write_environment_file(
    environment_dir: &Path,
    env_file: EnvironmentFile,
) -> miette::Result<PathBuf> {
    let path = environment_file_path(environment_dir);

    let parent = path
        .parent()
        .expect("There should already be a conda-meta folder");

    match fs_err::create_dir_all(parent).into_diagnostic() {
        Ok(_) => {
            // Using json as it's easier to machine read it.
            let contents = serde_json::to_string_pretty(&env_file).into_diagnostic()?;
            match fs_err::write(&path, contents).into_diagnostic() {
                Ok(_) => {
                    tracing::debug!("Wrote environment file to: {:?}", path);
                }
                Err(e) => tracing::debug!(
                    "Unable to write environment file to: {:?} => {:?}",
                    path,
                    e.root_cause().to_string()
                ),
            };
            Ok(path)
        }
        Err(e) => {
            tracing::debug!("Unable to create conda-meta folder to: {:?}", path);
            Err(e)
        }
    }
}

/// Reading the environment file of the environment.
/// Removing it if it's not valid.
pub(crate) fn read_environment_file(
    environment_dir: &Path,
) -> miette::Result<Option<EnvironmentFile>> {
    let path = environment_file_path(environment_dir);

    let contents = match fs_err::read_to_string(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::debug!("Environment file not yet found at: {:?}", path);
            return Ok(None);
        }
        Err(e) => {
            tracing::debug!(
                "Failed to read environment file at: {:?}, error: {}, will try to remove it.",
                path,
                e
            );
            let _ = fs_err::remove_file(&path);
            return Err(e).into_diagnostic();
        }
    };
    let env_file: EnvironmentFile = match serde_json::from_str(&contents) {
        Ok(env_file) => env_file,
        Err(e) => {
            tracing::debug!(
                "Failed to read environment file at: {:?}, error: {}, will try to remove it.",
                path,
                e
            );
            let _ = fs_err::remove_file(&path);
            return Ok(None);
        }
    };

    Ok(Some(env_file))
}

/// Runs the following checks to make sure the project is in a sane state:
///     1. It verifies that the prefix location is unchanged.
///     2. It verifies that the system requirements are met.
///     3. It verifies the absence of the `env` folder.
///     4. It verifies that the prefix contains a `.gitignore` file.
pub async fn sanity_check_project(project: &Workspace) -> miette::Result<()> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.environments_dir().as_path()).await?;

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

    ensure_pixi_directory_and_gitignore(project.pixi_dir().as_path()).await?;

    Ok(())
}

/// Extract [`GitSpec`] requirements from the project dependencies.
pub fn extract_git_requirements_from_project(project: &Workspace) -> Vec<GitSpec> {
    let mut requirements = Vec::new();

    for env in project.environments() {
        let env_platforms = env.platforms();
        for platform in env_platforms {
            let dependencies = env.combined_dependencies(Some(platform));
            let pypi_dependencies = env.pypi_dependencies(Some(platform));
            for (_, dep_spec) in dependencies {
                for spec in dep_spec {
                    if let PixiSpec::Git(spec) = spec {
                        requirements.push(spec.clone());
                    }
                }
            }

            for (_, pypi_spec) in pypi_dependencies {
                for spec in pypi_spec {
                    if let PyPiRequirement::Git { url, .. } = spec {
                        requirements.push(url);
                    }
                }
            }
        }
    }

    requirements
}

/// Store credentials from [`GitSpec`] requirements.
pub fn store_credentials_from_requirements(git_requirements: Vec<GitSpec>) {
    for spec in git_requirements {
        store_credentials_from_url(&spec.git);
    }
}

/// Extract any credentials that are defined on the project dependencies
/// themselves. While we don't store plaintext credentials in the `pixi.lock`,
/// we do respect credentials that are defined in the `pixi.toml` or
/// `pyproject.toml`.
pub async fn store_credentials_from_project(project: &Workspace) -> miette::Result<()> {
    for env in project.environments() {
        let env_platforms = env.platforms();
        for platform in env_platforms {
            let dependencies = env.combined_dependencies(Some(platform));
            for (_, dep_spec) in dependencies {
                for spec in dep_spec {
                    if let PixiSpec::Git(spec) = spec {
                        store_credentials_from_url(&spec.git);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Ensure that the `.pixi/` directory exists and contains a `.gitignore` file.
/// If the directory doesn't exist, create it.
/// If the `.gitignore` file doesn't exist, create it with a '*' pattern.
async fn ensure_pixi_directory_and_gitignore(pixi_dir: &Path) -> miette::Result<()> {
    let gitignore_path = pixi_dir.join(".gitignore");

    // Create the `.pixi/` directory if it doesn't exist
    if !pixi_dir.exists() {
        tokio::fs::create_dir_all(&pixi_dir)
            .await
            .into_diagnostic()
            .wrap_err(format!(
                "Failed to create .pixi/ directory at {}",
                pixi_dir.display()
            ))?;
    }

    // Create or check the .gitignore file
    if !gitignore_path.exists() {
        tokio::fs::write(&gitignore_path, "*\n")
            .await
            .into_diagnostic()
            .wrap_err(format!(
                "Failed to create .gitignore file at {}",
                gitignore_path.display()
            ))?;
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
    pub(crate) fn allows_lock_file_updates(self) -> bool {
        match self {
            LockFileUsage::Update => true,
            LockFileUsage::Locked | LockFileUsage::Frozen => false,
        }
    }

    /// Returns true if the lock-file should be checked if it is out of date.
    pub(crate) fn should_check_if_out_of_date(self) -> bool {
        match self {
            LockFileUsage::Update | LockFileUsage::Locked => true,
            LockFileUsage::Frozen => false,
        }
    }
}

/// Update the prefix if it doesn't exist or if it is not up-to-date.
///
/// The `sparse_repo_data` is used when the lock-file is update. We pass it into
/// this function to make sure the data is not loaded twice since the repodata
/// takes up a lot of memory and takes a while to load. If `sparse_repo_data` is
/// `None` it will be downloaded. If the lock-file is not updated, the
/// `sparse_repo_data` is ignored.
pub async fn get_update_lock_file_and_prefix<'env>(
    environment: &Environment<'env>,
    update_mode: UpdateMode,
    update_lock_file_options: UpdateLockFileOptions,
) -> miette::Result<(LockFileDerivedData<'env>, Prefix)> {
    let current_platform = environment.best_platform();
    let project = environment.workspace();

    // Do not install if the platform is not supported
    let mut no_install = update_lock_file_options.no_install;
    if !no_install && !environment.platforms().contains(&current_platform) {
        tracing::warn!("Not installing dependency on current platform: ({current_platform}) as it is not part of this project's supported platforms.");
        no_install = true;
    }

    // Make sure the project is in a sane state
    sanity_check_project(project).await?;

    // Store the git credentials from the git requirements
    let requirements = extract_git_requirements_from_project(project);
    store_credentials_from_requirements(requirements);

    // Ensure that the lock-file is up-to-date
    let mut lock_file = project
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: update_lock_file_options.lock_file_usage,
            no_install,
            max_concurrent_solves: update_lock_file_options.max_concurrent_solves,
        })
        .await?;

    // Get the prefix from the lock-file.
    let prefix = if no_install {
        Prefix::new(environment.dir())
    } else {
        lock_file.prefix(environment, update_mode).await?
    };

    Ok((lock_file, prefix))
}

pub type PerEnvironment<'p, T> = HashMap<Environment<'p>, T>;
pub type PerGroup<'p, T> = HashMap<GroupedEnvironment<'p>, T>;
pub type PerEnvironmentAndPlatform<'p, T> = PerEnvironment<'p, HashMap<Platform, T>>;
pub type PerGroupAndPlatform<'p, T> = PerGroup<'p, HashMap<Platform, T>>;
