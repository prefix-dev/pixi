use std::{
    collections::HashMap,
    convert::identity,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use dialoguer::theme::ColorfulTheme;
use distribution_types::{InstalledDist, Name};
use fancy_display::FancyDisplay;
use futures::{stream, StreamExt, TryStreamExt};
use indicatif::ProgressBar;
use itertools::{Either, Itertools};
use miette::{IntoDiagnostic, WrapErr};
use parking_lot::Mutex;
use pixi_build_frontend::CondaBuildReporter;
use pixi_consts::consts;
use pixi_manifest::{EnvironmentName, FeaturesExt, SystemRequirements};
use pixi_progress::{await_in_progress, global_multi_progress};
use pixi_record::PixiRecord;
use rattler::{
    install::{DefaultProgressFormatter, IndicatifReporter, Installer, PythonInfo, Transaction},
    package_cache::PackageCache,
};
use rattler_conda_types::{GenericVirtualPackage, Platform, PrefixRecord, RepoDataRecord};
use rattler_lock::{PypiIndexes, PypiPackageData, PypiPackageEnvironmentData};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::Semaphore;
use url::Url;

use crate::{
    build::{BuildContext, BuildReporter},
    install_pypi,
    lock_file::{UpdateLockFileOptions, UvResolutionContext},
    prefix::Prefix,
    project::{grouped_environment::GroupedEnvironment, Environment, HasProjectRef},
    rlimit::try_increase_rlimit_to_sensible,
    Project,
};

/// Verify the location of the prefix folder is not changed so the applied
/// prefix path is still valid. Errors when there is a file system error or the
/// path does not align with the defined prefix. Returns false when the file is
/// not present.
pub async fn verify_prefix_location_unchanged(environment_dir: &Path) -> miette::Result<()> {
    let prefix_file = environment_dir
        .join("conda-meta")
        .join(consts::PREFIX_FILE_NAME);

    tracing::info!(
        "verifying prefix location is unchanged, with prefix file: {}",
        prefix_file.display()
    );

    match std::fs::read_to_string(prefix_file.clone()) {
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
        .map_or(None, identity);
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

/// Create the prefix location file.
/// Give it the environment path to place it.
fn create_prefix_location_file(environment_dir: &Path) -> miette::Result<()> {
    let prefix_file_path = environment_dir
        .join("conda-meta")
        .join(consts::PREFIX_FILE_NAME);
    tracing::info!("Creating prefix file at: {}", prefix_file_path.display());

    let parent_dir = prefix_file_path.parent().ok_or_else(|| {
        miette::miette!(
            "Cannot find parent directory of '{}'",
            prefix_file_path.display()
        )
    })?;

    if parent_dir.exists() {
        let contents = parent_dir.to_string_lossy();

        let path = Path::new(&prefix_file_path);
        // Read existing contents to determine if an update is necessary
        if path.exists() {
            let existing_contents = std::fs::read_to_string(path).into_diagnostic()?;
            if existing_contents == contents {
                tracing::info!("No update needed for the prefix file.");
                return Ok(());
            }
        }

        // Write new contents to the prefix file
        std::fs::write(path, &*contents).into_diagnostic()?;
        tracing::info!("Prefix file updated with: '{}'.", contents);
    }
    Ok(())
}

/// Create the conda-meta/history.
/// This file is needed for `conda run -p .pixi/envs/<env>` to work.
fn create_history_file(environment_dir: &Path) -> miette::Result<()> {
    let history_file = environment_dir.join("conda-meta").join("history");

    tracing::info!(
        "Checking if history file exists: {}",
        history_file.display()
    );

    let binding = history_file.clone();
    let parent = binding
        .parent()
        .ok_or_else(|| miette::miette!("cannot find parent of '{}'", binding.display()))?;

    if parent.exists() && !history_file.exists() {
        tracing::info!("Creating history file: {}", history_file.display());
        std::fs::write(
            history_file,
            "// not relevant for pixi but for `conda run -p`",
        )
        .into_diagnostic()?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub(crate) struct EnvironmentFile {
    pub(crate) manifest_path: PathBuf,
    pub(crate) environment_name: String,
    pub(crate) pixi_version: String,
}
/// Write information about the environment to a file in the environment
/// directory. This can be useful for other tools that only know the environment
/// directory to find the original project.
pub(crate) fn write_environment_file(
    environment_dir: &Path,
    env_file: EnvironmentFile,
) -> miette::Result<PathBuf> {
    let path = environment_dir
        .join("conda-meta")
        .join(consts::ENVIRONMENT_FILE_NAME);

    let parent = path
        .parent()
        .expect("There should already be a conda-meta folder");

    std::fs::create_dir_all(parent).into_diagnostic()?;

    // Using json as it's easier to machine read it.
    let contents = serde_json::to_string_pretty(&env_file).into_diagnostic()?;
    std::fs::write(&path, contents).into_diagnostic()?;

    tracing::debug!("Wrote environment file to: {:?}", path);

    Ok(path)
}

/// Runs the following checks to make sure the project is in a sane state:
///     1. It verifies that the prefix location is unchanged.
///     2. It verifies that the system requirements are met.
///     3. It verifies the absence of the `env` folder.
pub async fn sanity_check_project(project: &Project) -> miette::Result<()> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(project.default_environment().dir().as_path()).await?;

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
pub async fn update_prefix(
    environment: &Environment<'_>,
    lock_file_usage: LockFileUsage,
    mut no_install: bool,
) -> miette::Result<()> {
    let current_platform = environment.best_platform();
    let project = environment.project();

    // Do not install if the platform is not supported
    if !no_install && !environment.platforms().contains(&current_platform) {
        tracing::warn!("Not installing dependency on current platform: ({current_platform}) as it is not part of this project's supported platforms.");
        no_install = true;
    }

    // Make sure the project is in a sane state
    sanity_check_project(project).await?;

    // Ensure that the lock-file is up-to-date
    let mut lock_file = project
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage,
            no_install,
            ..UpdateLockFileOptions::default()
        })
        .await?;

    // Get the locked environment from the lock-file.
    if !no_install {
        lock_file.prefix(environment).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// TODO: refactor args into struct
pub async fn update_prefix_pypi(
    environment_name: &EnvironmentName,
    prefix: &Prefix,
    _platform: Platform,
    pixi_records: &[PixiRecord],
    pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
    status: &PythonStatus,
    system_requirements: &SystemRequirements,
    uv_context: &UvResolutionContext,
    pypi_indexes: Option<&PypiIndexes>,
    environment_variables: &HashMap<String, String>,
    lock_file_dir: &Path,
    platform: Platform,
    non_isolated_packages: Option<Vec<String>>,
) -> miette::Result<()> {
    // If we have changed interpreter, we need to uninstall all site-packages from
    // the old interpreter We need to do this before the pypi prefix update,
    // because that requires a python interpreter.
    let python_info = match status {
        // If the python interpreter is removed, we need to uninstall all `pixi-uv` site-packages.
        // And we don't need to continue with the rest of the pypi prefix update.
        PythonStatus::Removed { old } => {
            let site_packages_path = prefix.root().join(&old.site_packages_path);
            if site_packages_path.exists() {
                uninstall_outdated_site_packages(&site_packages_path).await?;
            }
            return Ok(());
        }
        // If the python interpreter is changed, we need to uninstall all site-packages from the old
        // interpreter. And we continue the function to update the pypi packages.
        PythonStatus::Changed { old, new } => {
            // In windows the site-packages path stays the same, so we don't need to
            // uninstall the site-packages ourselves.
            if old.site_packages_path != new.site_packages_path {
                let site_packages_path = prefix.root().join(&old.site_packages_path);
                if site_packages_path.exists() {
                    uninstall_outdated_site_packages(&site_packages_path).await?;
                }
            }
            new
        }
        // If the python interpreter is unchanged, and there are no pypi packages to install, we
        // need to remove the site-packages. And we don't need to continue with the rest of
        // the pypi prefix update.
        PythonStatus::Unchanged(info) | PythonStatus::Added { new: info } => {
            if pypi_records.is_empty() {
                let site_packages_path = prefix.root().join(&info.site_packages_path);
                if site_packages_path.exists() {
                    uninstall_outdated_site_packages(&site_packages_path).await?;
                }
                return Ok(());
            }
            info
        }
        // We can skip the pypi prefix update if there is not python interpreter in the environment.
        PythonStatus::DoesNotExist => {
            return Ok(());
        }
    };

    // Install and/or remove python packages
    await_in_progress(
        format!(
            "updating pypi packages in '{}'",
            environment_name.fancy_display()
        ),
        |_| {
            install_pypi::update_python_distributions(
                lock_file_dir,
                prefix,
                pixi_records,
                pypi_records,
                &python_info.path,
                system_requirements,
                uv_context,
                pypi_indexes,
                environment_variables,
                platform,
                non_isolated_packages,
            )
        },
    )
    .await
}

/// If the python interpreter is outdated, we need to uninstall all outdated
/// site packages. from the old interpreter.
/// TODO: optimize this by recording the installation of the site-packages to
/// check if this is needed.
async fn uninstall_outdated_site_packages(site_packages: &Path) -> miette::Result<()> {
    // Check if the old interpreter is outdated
    let mut installed = vec![];
    for entry in std::fs::read_dir(site_packages).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        if entry.file_type().into_diagnostic()?.is_dir() {
            let path = entry.path();

            let installed_dist = InstalledDist::try_from_path(&path);
            let Ok(installed_dist) = installed_dist else {
                continue;
            };

            if let Some(installed_dist) = installed_dist {
                // If we can't get the installer, we can't be certain that we have installed it
                let installer = match installed_dist.installer() {
                    Ok(installer) => installer,
                    Err(e) => {
                        tracing::warn!(
                            "could not get installer for {}: {}, will not remove distribution",
                            installed_dist.name(),
                            e
                        );
                        continue;
                    }
                };

                // Only remove if have actually installed it
                // by checking the installer
                if installer.unwrap_or_default() == consts::PIXI_UV_INSTALLER {
                    installed.push(installed_dist);
                }
            }
        }
    }

    // Uninstall all packages in old site-packages directory
    for dist_info in installed {
        let _summary = uv_installer::uninstall(&dist_info)
            .await
            .expect("uninstallation of old site-packages failed");
    }

    Ok(())
}

#[derive(Clone, Debug)]
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
    pub(crate) fn from_transaction(
        transaction: &Transaction<PrefixRecord, RepoDataRecord>,
    ) -> Self {
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

    /// Returns the info of the current situation (e.g. after the transaction
    /// completed).
    pub(crate) fn current_info(&self) -> Option<&PythonInfo> {
        match self {
            PythonStatus::Changed { new, .. }
            | PythonStatus::Unchanged(new)
            | PythonStatus::Added { new } => Some(new),
            PythonStatus::Removed { .. } | PythonStatus::DoesNotExist => None,
        }
    }

    /// Returns the location of the python interpreter relative to the root of
    /// the prefix.
    pub(crate) fn location(&self) -> Option<&Path> {
        Some(&self.current_info()?.path)
    }
}

struct CondaBuildProgress {
    main_progress: ProgressBar,
    build_progress: Mutex<Vec<(String, ProgressBar)>>,
}

impl CondaBuildProgress {
    fn new(num_packages: u64) -> Self {
        // Create a new progress bar.
        let pb = ProgressBar::hidden();
        pb.set_length(num_packages);
        let pb = pixi_progress::global_multi_progress().add(pb);
        pb.set_style(pixi_progress::default_progress_style());
        // Building the package
        pb.set_prefix("building packages");
        pb.enable_steady_tick(Duration::from_millis(100));

        Self {
            main_progress: pb,
            build_progress: Mutex::new(Vec::default()),
        }
    }
}

impl CondaBuildProgress {
    /// Associate a progress bar with a build identifier, and get a build id back
    pub fn associate(&self, identifier: &str) -> usize {
        let mut locked = self.build_progress.lock();
        let after = if locked.is_empty() {
            &self.main_progress
        } else {
            &locked.last().unwrap().1
        };

        let pb = pixi_progress::global_multi_progress().insert_after(after, ProgressBar::hidden());

        locked.push((identifier.to_owned(), pb));
        locked.len() - 1
    }

    pub fn end_progress_for(&self, build_id: usize, alternative_message: Option<String>) {
        self.main_progress.inc(1);
        if self.main_progress.position()
            == self
                .main_progress
                .length()
                .expect("expected length to be set for progress")
        {
            self.main_progress.finish_and_clear();
            // Clear all the build progress bars
            for (_, pb) in self.build_progress.lock().iter() {
                pb.finish_and_clear();
            }
            return;
        }
        let locked = self.build_progress.lock();

        // Finish the build progress bar
        let (identifier, pb) = locked.get(build_id).unwrap();
        // If there is an alternative message, use that
        let msg = if let Some(msg) = alternative_message {
            pb.set_style(indicatif::ProgressStyle::with_template("    {msg}").unwrap());
            msg
        } else {
            // Otherwise show the default message
            pb.set_style(
                indicatif::ProgressStyle::with_template("    {msg} in {elapsed}").unwrap(),
            );
            "built".to_string()
        };
        pb.finish_with_message(format!("✔ {msg}: {identifier}"));
    }
}

impl CondaBuildReporter for CondaBuildProgress {
    fn on_build_start(&self, build_id: usize) -> usize {
        // Actually show the progress
        let locked = self.build_progress.lock();
        let (identifier, pb) = locked.get(build_id).unwrap();
        let template =
            indicatif::ProgressStyle::with_template("    {spinner:.green} {msg} {elapsed}")
                .unwrap();
        pb.set_style(template);
        pb.set_message(format!("building {identifier}"));
        pb.enable_steady_tick(Duration::from_millis(100));
        // We keep operation and build id the same
        build_id
    }

    fn on_build_end(&self, operation: usize) {
        self.end_progress_for(operation, None);
    }

    fn on_build_output(&self, _operation: usize, line: String) {
        self.main_progress.suspend(|| eprintln!("{}", line));
    }
}

impl BuildReporter for CondaBuildProgress {
    fn on_build_cached(&self, build_id: usize) {
        self.end_progress_for(build_id, Some("cached".to_string()));
    }

    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter> {
        self.clone()
    }
}

/// Updates the environment to contain the packages from the specified lock-file
#[allow(clippy::too_many_arguments)]
pub async fn update_prefix_conda(
    prefix: &Prefix,
    package_cache: PackageCache,
    authenticated_client: ClientWithMiddleware,
    installed_packages: Vec<PrefixRecord>,
    pixi_records: Vec<PixiRecord>,
    virtual_packages: Vec<GenericVirtualPackage>,
    channels: Vec<Url>,
    platform: Platform,
    progress_bar_message: &str,
    progress_bar_prefix: &str,
    io_concurrency_limit: Arc<Semaphore>,
    build_context: BuildContext,
) -> miette::Result<PythonStatus> {
    // Try to increase the rlimit to a sensible value for installation.
    try_increase_rlimit_to_sensible();

    let (mut repodata_records, source_records): (Vec<_>, Vec<_>) = pixi_records
        .into_iter()
        .partition_map(|record| match record {
            PixiRecord::Binary(record) => Either::Left(record),
            PixiRecord::Source(record) => Either::Right(record),
        });

    let mut progress_reporter = None;
    let source_records_length = source_records.len();
    // Build conda packages out of the source records
    let mut processed_source_packages = stream::iter(source_records)
        .map(Ok)
        .and_then(|record| {
            // If we don't have a progress reporter, create one
            // This is done so that the progress bars are not displayed if there are no source packages
            let progress_reporter = progress_reporter
                .get_or_insert_with(|| {
                    Arc::new(CondaBuildProgress::new(source_records_length as u64))
                })
                .clone();
            let build_id = progress_reporter.associate(record.package_record.name.as_source());
            let build_context = &build_context;
            let channels = &channels;
            let virtual_packages = &virtual_packages;
            async move {
                build_context
                    .build_source_record(
                        &record,
                        channels,
                        platform,
                        virtual_packages.clone(),
                        virtual_packages.clone(),
                        progress_reporter.clone(),
                        build_id,
                    )
                    .await
            }
        })
        .try_collect::<Vec<RepoDataRecord>>()
        .await?;

    // Extend the repodata records with the built packages
    repodata_records.append(&mut processed_source_packages);

    // Execute the operations that are returned by the solver.
    let result = await_in_progress(
        format!("{progress_bar_prefix}{progress_bar_message}",),
        |pb| async {
            Installer::new()
                .with_download_client(authenticated_client)
                .with_io_concurrency_semaphore(io_concurrency_limit)
                .with_execute_link_scripts(false)
                .with_installed_packages(installed_packages)
                .with_target_platform(platform)
                .with_package_cache(package_cache)
                .with_reporter(
                    IndicatifReporter::builder()
                        .with_multi_progress(global_multi_progress())
                        .with_placement(rattler::install::Placement::After(pb))
                        .with_formatter(
                            DefaultProgressFormatter::default()
                                .with_prefix(format!("{progress_bar_prefix}  ")),
                        )
                        .clear_when_done(true)
                        .finish(),
                )
                .install(prefix.root(), repodata_records)
                .await
                .into_diagnostic()
        },
    )
    .await?;

    // Mark the location of the prefix
    create_prefix_location_file(prefix.root())?;
    create_history_file(prefix.root())?;

    // Determine if the python version changed.
    Ok(PythonStatus::from_transaction(&result.transaction))
}

pub type PerEnvironment<'p, T> = HashMap<Environment<'p>, T>;
pub type PerGroup<'p, T> = HashMap<GroupedEnvironment<'p>, T>;
pub type PerEnvironmentAndPlatform<'p, T> = PerEnvironment<'p, HashMap<Platform, T>>;
pub type PerGroupAndPlatform<'p, T> = PerGroup<'p, HashMap<Platform, T>>;
