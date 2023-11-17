use crate::{
    consts, default_authenticated_client, install, lock_file, prefix::Prefix, progress,
    progress::ProgressBarMessageFormatter,
    virtual_packages::verify_current_platform_has_required_virtual_packages, Project,
};
use futures::{stream, Stream, StreamExt, TryFutureExt, TryStreamExt};
use indexmap::IndexSet;
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, LabeledSpan};

use rattler::install::Transaction;
use rattler_conda_types::{Platform, PrefixRecord, RepoDataRecord};
use rattler_lock::{CondaLock, LockedDependency, PipLockedDependency};
use rip::{
    tags::WheelTag, Artifact, ArtifactHashes, ArtifactInfo, ArtifactName, Distribution,
    InstallPaths, PackageDb, ParseArtifactNameError, UnpackWheelOptions, Wheel, WheelName,
};
use std::{io::ErrorKind, path::Path, str::FromStr, time::Duration};
use tokio::task::JoinError;

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

/// Returns the prefix associated with the given environment. If the prefix doesn't exist or is not
/// up to date it is updated.
/// Use `frozen` or `locked` to skip the update of the lockfile. Use frozen when you don't even want
/// to check the lockfile status.
pub async fn get_up_to_date_prefix(
    project: &Project,
    frozen: bool,
    locked: bool,
) -> miette::Result<Prefix> {
    // Sanity check of prefix location
    verify_prefix_location_unchanged(
        project
            .environment_dir()
            .join(consts::PREFIX_FILE_NAME)
            .as_path(),
    )?;

    // Make sure the project supports the current platform
    let platform = Platform::current();
    if !project.platforms().contains(&platform) {
        let span = project.manifest.project.platforms.span();
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
        .with_source_code(project.source()));
    }

    // Make sure the system requirements are met
    verify_current_platform_has_required_virtual_packages(project)?;

    // Start loading the installed packages in the background
    let prefix = Prefix::new(project.root().join(".pixi/env"))?;
    let installed_packages_future = {
        let prefix = prefix.clone();
        tokio::spawn(async move { prefix.find_installed_packages(None).await })
    };

    // Update the lock-file if it is out of date.
    if frozen && locked {
        miette::bail!("Frozen and Locked can't be true at the same time, as using frozen will ignore the locked variable.");
    }
    if frozen && !project.lock_file_path().is_file() {
        miette::bail!("No lockfile available, can't do a frozen installation.");
    }

    let mut lock_file = lock_file::load_lock_file(project).await?;

    if !frozen && !lock_file::lock_file_up_to_date(project, &lock_file)? {
        if locked {
            miette::bail!("Lockfile not up-to-date with the project");
        }
        lock_file = lock_file::update_lock_file(project, lock_file, None).await?
    }

    // Update the environment
    update_prefix(
        project.pypi_package_db()?,
        &prefix,
        installed_packages_future.await.into_diagnostic()??,
        &lock_file,
        platform,
    )
    .await?;

    Ok(prefix)
}

/// Updates the environment to contain the packages from the specified lock-file
pub async fn update_prefix(
    package_db: &PackageDb,
    prefix: &Prefix,
    installed_packages: Vec<PrefixRecord>,
    lock_file: &CondaLock,
    platform: Platform,
) -> miette::Result<()> {
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

    // Remove python packages from a previous python distribution if the python version changed.
    remove_old_python_distributions(prefix, platform, &transaction)?;

    // Install and/or remove python packages
    progress::await_in_progress(
        "updating python packages",
        update_python_distributions(package_db, &prefix, lock_file, platform, &transaction),
    )
    .await?;

    // Mark the location of the prefix
    create_prefix_location_file(
        &prefix
            .root()
            .parent()
            .map(|p| p.join(consts::PREFIX_FILE_NAME))
            .ok_or_else(|| miette::miette!("we should be able to create a prefix file name."))?,
    )
    .with_context(|| "failed to create prefix location file.".to_string())?;

    Ok(())
}

/// Installs and/or remove python distributions.
async fn update_python_distributions(
    package_db: &PackageDb,
    prefix: &&Prefix,
    lock_file: &CondaLock,
    platform: Platform,
    transaction: &Transaction<PrefixRecord, RepoDataRecord>,
) -> miette::Result<()> {
    // Get the python info from the transaction
    let Some(python_info) = transaction.python_info.as_ref() else {
        return Ok(());
    };

    // Determine where packages would have been installed
    let install_paths = InstallPaths::for_venv(
        (
            python_info.short_version.0 as u32,
            python_info.short_version.1 as u32,
            0,
        ),
        platform.is_windows(),
    );

    // Determine the current python distributions in those locations
    let current_python_packages = rip::find_distributions_in_venv(prefix.root(), &install_paths)
        .into_diagnostic()
        .context(
            "failed to locate python packages that have not been installed as conda packages",
        )?;

    // Determine the python packages that are part of the lock-file
    let python_packages = lock_file
        .get_packages_by_platform(platform)
        .filter(|p| p.is_pip())
        .collect_vec();

    // Determine the python packages to remove before we start installing anything new. If the
    // python version changed between installations we will have to remove any previous distribution
    // regardless.
    let (python_distributions_to_remove, python_distributions_to_install) =
        determine_python_distributions_to_remove_and_install(
            current_python_packages,
            python_packages,
        );

    // Start downloading the python packages that we want in the background.
    let (package_stream, package_stream_pb) =
        stream_python_artifacts(package_db, python_distributions_to_install.clone());

    // Remove python packages that need to be removed
    if !python_distributions_to_remove.is_empty() {
        let site_package_path = install_paths
            .site_packages()
            .expect("site-packages path must exist");

        for python_distribution in python_distributions_to_remove {
            tracing::info!(
                "uninstalling python package {}-{}",
                &python_distribution.name,
                &python_distribution.version
            );
            let relative_dist_info = python_distribution
                .dist_info
                .strip_prefix(site_package_path)
                .expect("the dist-info path must be a sub-path of the site-packages path");
            rip::uninstall::uninstall_distribution(&prefix.root().join(site_package_path), relative_dist_info)
                .into_diagnostic()
                .with_context(|| format!("could not uninstall python package {}-{}. Manually remove the `.pixi/env` folder and try again.", &python_distribution.name, &python_distribution.version))?;
        }
    }

    // Install the individual python packages that we want
    let package_install_pb =
        install_python_distributions(prefix, install_paths, package_stream).await?;

    // Clear any pending progress bar
    for pb in package_install_pb
        .into_iter()
        .chain(package_stream_pb.into_iter())
    {
        pb.finish_and_clear();
    }

    Ok(())
}

/// Concurrently installs python wheels as they become available.
async fn install_python_distributions(
    prefix: &Prefix,
    install_paths: InstallPaths,
    package_stream: impl Stream<Item = miette::Result<Wheel>> + Sized,
) -> miette::Result<Option<ProgressBar>> {
    // Determine the number of packages that we are going to install
    let len = {
        let (lower_bound, upper_bound) = package_stream.size_hint();
        upper_bound.unwrap_or(lower_bound)
    };
    if len == 0 {
        return Ok(None);
    }

    // Create a progress bar to show the progress of the installation
    let pb = progress::global_multi_progress().add(ProgressBar::new(len as u64));
    pb.set_style(progress::default_progress_style());
    pb.set_prefix("unpacking wheels");
    pb.enable_steady_tick(Duration::from_millis(100));

    // Create a message formatter to show the current operation
    let message_formatter = ProgressBarMessageFormatter::new(pb.clone());

    // Concurrently unpack the wheels as they become available in the stream.
    let install_pb = pb.clone();
    package_stream
        .try_for_each_concurrent(Some(20), move |wheel| {
            let install_paths = install_paths.clone();
            let root = prefix.root().to_path_buf();
            let message_formatter = message_formatter.clone();
            let pb = install_pb.clone();
            async move {
                let pb_task = message_formatter.start(wheel.name().to_string()).await;
                let unpack_result = tokio::task::spawn_blocking(move || {
                    wheel
                        .unpack(
                            &root,
                            &install_paths,
                            &UnpackWheelOptions {
                                installer: Some(env!("CARGO_PKG_NAME").into()),
                            },
                        )
                        .into_diagnostic()
                })
                .map_err(JoinError::try_into_panic)
                .await;

                pb_task.finish().await;
                pb.inc(1);

                match unpack_result {
                    Ok(unpack_result) => unpack_result,
                    Err(Ok(panic)) => std::panic::resume_unwind(panic),
                    Err(Err(e)) => Err(miette::miette!("{e}")),
                }
            }
        })
        .await?;

    // Update the progress bar
    pb.set_style(progress::finished_progress_style());
    pb.finish();

    Ok(Some(pb))
}

/// Creates a stream which downloads the specified python packages. The stream will download the
/// packages in parallel and yield them as soon as they become available.
fn stream_python_artifacts<'a>(
    package_db: &'a PackageDb,
    packages_to_download: Vec<&'a LockedDependency>,
) -> (
    impl Stream<Item = miette::Result<Wheel>> + 'a,
    Option<ProgressBar>,
) {
    if packages_to_download.is_empty() {
        return (stream::empty().left_stream(), None);
    }

    // Construct a progress bar to provide some indication on what is currently downloading.
    // TODO: It would be much nicer if we can provide more information with regards to the progress.
    //  For instance if we could also show at what speed the downloads are progressing or the total
    //  size of the downloads that would really help the user I think.
    let pb =
        progress::global_multi_progress().add(ProgressBar::new(packages_to_download.len() as u64));
    pb.set_style(progress::default_progress_style());
    pb.set_prefix("acquiring wheels");
    pb.enable_steady_tick(Duration::from_millis(100));

    // Construct a message formatter
    let message_formatter = ProgressBarMessageFormatter::new(pb.clone());

    let stream_pb = pb.clone();
    let total_packages = packages_to_download.len();
    let download_stream = stream::iter(packages_to_download)
        .map(move |package| {
            let pb = stream_pb.clone();
            let message_formatter = message_formatter.clone();
            async move {
                let pip_package = package
                    .as_pip()
                    .expect("must be a pip package at this point");

                // Determine the filename from the
                let filename = pip_package
                    .url
                    .path_segments()
                    .and_then(|s| s.last())
                    .expect("url is missing a path");
                let wheel_name = WheelName::from_str(filename)
                    .expect("failed to convert filename to wheel filename");

                // Log out intent to install this python package.
                tracing::info!("downloading python package {filename}");
                let pb_task = message_formatter.start(filename.to_string()).await;

                // Reconstruct the ArtifactInfo from the data in the lockfile.
                let artifact_info = ArtifactInfo {
                    filename: ArtifactName::Wheel(wheel_name),
                    url: pip_package.url.clone(),
                    hashes: pip_package.hash.as_ref().map(|hash| ArtifactHashes {
                        sha256: hash.sha256().cloned(),
                    }),
                    requires_python: pip_package
                        .requires_python
                        .as_ref()
                        .map(|p| p.parse())
                        .transpose()
                        .expect("the lock file contains an invalid 'requires_python` field"),
                    dist_info_metadata: Default::default(),
                    yanked: Default::default(),
                };

                // TODO: Maybe we should have a cache of wheels separate from the package_db. Since a
                //   wheel can just be identified by its hash or url.
                let wheel: Wheel = package_db.get_artifact(&artifact_info).await?;

                // Update the progress bar
                pb_task.finish().await;
                pb.inc(1);
                if pb.position() == total_packages as u64 {
                    pb.set_style(progress::finished_progress_style());
                    pb.finish();
                }

                Ok(wheel)
            }
        })
        .buffer_unordered(20)
        .right_stream();

    (download_stream, Some(pb))
}

/// If there was a previous version of python installed, remove any distribution installed in that
/// environment.
fn remove_old_python_distributions(
    prefix: &Prefix,
    platform: Platform,
    transaction: &Transaction<PrefixRecord, RepoDataRecord>,
) -> miette::Result<()> {
    // Determine if the current distribution is the same as the desired distribution.
    let Some(previous_python_installation) = transaction.current_python_info.as_ref() else {
        return Ok(());
    };
    if Some(previous_python_installation.short_version)
        == transaction.python_info.as_ref().map(|p| p.short_version)
    {
        return Ok(());
    }

    // Determine the current python distributions in its install locations
    let python_version = (
        previous_python_installation.short_version.0 as u32,
        previous_python_installation.short_version.1 as u32,
        0,
    );
    let install_paths = InstallPaths::for_venv(python_version, platform.is_windows());

    // Locate the packages that are installed in the previous environment
    let current_python_packages = rip::find_distributions_in_venv(prefix.root(), &install_paths)
        .into_diagnostic()
        .with_context(|| format!("failed to determine the python packages installed for a previous version of python ({}.{})", python_version.0, python_version.1))?
        .into_iter().filter(|d| d.installer.as_deref() != Some("conda")).collect_vec();

    let pb = progress::global_multi_progress()
        .add(ProgressBar::new(current_python_packages.len() as u64));
    pb.set_style(progress::default_progress_style());
    pb.set_message("removing old python packages");
    pb.enable_steady_tick(Duration::from_millis(100));

    // Remove the python packages
    let site_package_path = install_paths
        .site_packages()
        .expect("site-packages path must exist");
    for python_package in current_python_packages {
        tracing::info!(
            "uninstalling python package from previous python version {}-{}",
            &python_package.name,
            &python_package.version
        );

        pb.set_message(format!(
            "{} {}",
            &python_package.name, &python_package.version
        ));

        let relative_dist_info = python_package
            .dist_info
            .strip_prefix(site_package_path)
            .expect("the dist-info path must be a sub-path of the site-packages path");
        rip::uninstall::uninstall_distribution(&prefix.root().join(site_package_path), relative_dist_info)
            .into_diagnostic()
            .with_context(|| format!("could not uninstall python package {}-{}. Manually remove the `.pixi/env` folder and try again.", &python_package.name, &python_package.version))?;

        pb.inc(1);
    }

    Ok(())
}

/// Determine which python packages we can leave untouched and which python packages should be
/// removed.
fn determine_python_distributions_to_remove_and_install(
    mut current_python_packages: Vec<Distribution>,
    desired_python_packages: Vec<&LockedDependency>,
) -> (Vec<Distribution>, Vec<&LockedDependency>) {
    // Determine the artifact tags associated with the locked dependencies.
    let mut desired_python_packages = extract_locked_tags(desired_python_packages);

    // Any package that is currently installed that is not part of the locked dependencies should be
    // removed. So we keep it in the `current_python_packages` list.
    // Any package that is in the currently installed list that is NOT found in the lockfile is
    // retained in the list to mark it for removal.
    current_python_packages.retain(|current_python_packages| {
        if let Some(found_desired_packages_idx) =
            desired_python_packages
                .iter()
                .position(|(pkg, artifact_name)| {
                    does_installed_match_locked_package(
                        current_python_packages,
                        (pkg, artifact_name.as_ref()),
                    )
                })
        {
            // Remove from the desired list of packages to install & from the packages to uninstall.
            desired_python_packages.remove(found_desired_packages_idx);
            false
        } else {
            // If this package was installed as a conda package we should not remove it.
            current_python_packages.installer.as_deref() != Some("conda")
        }
    });

    (
        current_python_packages,
        desired_python_packages
            .into_iter()
            .map(|(pkg, _)| pkg)
            .collect(),
    )
}

/// Determine the wheel tags for the locked dependencies. These are extracted by looking at the url
/// of the locked dependency. The filename of the URL is converted to a wheel name and the tags are
/// extract from that.
///
/// If the locked dependency is not a wheel distribution `None` is returned for the tags. If the
/// the wheel name could not be parsed `None` is returned for the tags and a warning is emitted.
fn extract_locked_tags(
    desired_python_packages: Vec<&LockedDependency>,
) -> Vec<(&LockedDependency, Option<IndexSet<WheelTag>>)> {
    desired_python_packages
        .into_iter()
        .map(|pkg| {
            let Some(pip) = pkg.as_pip() else { return (pkg, None); };
            match pip.artifact_name().as_ref().map(|name| name.as_wheel()) {
                Ok(Some(name)) => (pkg, Some(IndexSet::from_iter(name.all_tags_iter()))),
                Ok(None) => (pkg, None),
                Err(err) => {
                    tracing::warn!(
                        "failed to determine the artifact name of the python package {}-{}. Could not determine the name from the url {}: {err}",
                        &pkg.name, pkg.version, &pip.url);
                    (pkg, None)
                }
            }
        })
        .collect()
}

/// Returns true if the installed python package matches the locked python package. If that is the
/// case we can assume that the locked python package is already installed.
fn does_installed_match_locked_package(
    installed_python_package: &Distribution,
    locked_python_package: (&LockedDependency, Option<&IndexSet<WheelTag>>),
) -> bool {
    let (pkg, artifact_tags) = locked_python_package;

    // Match on name and version
    if pkg.name != installed_python_package.name.as_str()
        || pep440_rs::Version::from_str(&pkg.version).ok().as_ref()
            != Some(&installed_python_package.version)
    {
        return false;
    }

    // Now match on the type of the artifact
    match (artifact_tags, &installed_python_package.tags) {
        (None, _) | (_, None) => {
            // One, or both, of the artifacts are not a wheel distribution so we cannot
            // currently compare them. In that case we always just reinstall.
            // TODO: Maybe log some info here?
            // TODO: Add support for more distribution types.
            false
        }
        (Some(locked_tags), Some(installed_tags)) => locked_tags == installed_tags,
    }
}

trait PipLockedDependencyExt {
    /// Returns the artifact name of the locked dependency.
    fn artifact_name(&self) -> Result<ArtifactName, ParseArtifactNameError>;
}

impl PipLockedDependencyExt for PipLockedDependency {
    fn artifact_name(&self) -> Result<ArtifactName, ParseArtifactNameError> {
        self.url.path_segments().and_then(|s| s.last()).map_or(
            Err(ParseArtifactNameError::InvalidName),
            ArtifactName::from_str,
        )
    }
}
