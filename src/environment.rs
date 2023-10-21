use crate::{
    default_authenticated_client, install, lock_file, prefix::Prefix, progress::await_in_progress,
    virtual_packages::verify_current_platform_has_required_virtual_packages, Project,
};
use indexmap::IndexSet;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, LabeledSpan};
use rattler::install::{PythonInfo, Transaction};
use rattler_conda_types::{Platform, PrefixRecord};
use rattler_lock::{CondaLock, LockedDependency, PipLockedDependency};
use rip::tags::WheelTag;
use rip::{
    ArtifactHashes, ArtifactInfo, ArtifactName, Distribution, FindDistributionError, InstallPaths,
    PackageDb, ParseArtifactNameError, UnpackWheelOptions, Wheel, WheelName,
};
use std::str::FromStr;

/// Returns the prefix associated with the given environment. If the prefix doesn't exist or is not
/// up to date it is updated.
/// Use `frozen` or `locked` to skip the update of the lockfile. Use frozen when you don't even want
/// to check the lockfile status.
pub async fn get_up_to_date_prefix(
    project: &Project,
    frozen: bool,
    locked: bool,
) -> miette::Result<Prefix> {
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
        project.python_package_db()?,
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
    let transaction = Transaction::from_current_and_desired(
        installed_packages,
        lock_file
            .get_conda_packages_by_platform(platform)
            .into_diagnostic()?,
        platform,
    )
    .into_diagnostic()?;

    // Determine currently installed python packages
    let current_python_distributions = transaction
        .current_python_info
        .as_ref()
        .map(|py_info| find_externally_installed_python_distributions(prefix, platform, py_info))
        .transpose()
        .into_diagnostic()
        .context("failed to locate python packages that have not been installed as conda packages")?
        .unwrap_or_default();

    // Log some information about these packages
    tracing::debug!(
        "found the following python packages in the environment:\n{}",
        current_python_distributions
            .iter()
            .format_with("\n", |d, f| f(&format_args!(
                "- {} (installed by {})",
                d.name,
                d.installer.as_deref().unwrap_or("?")
            )))
    );

    let python_version = transaction
        .python_info
        .clone()
        .map(|py| (py.short_version.0 as u32, py.short_version.1 as u32));

    // Determine the python packages that are part of the lock-file
    let python_packages = lock_file
        .get_packages_by_platform(platform)
        .filter(|p| p.is_pip())
        .collect_vec();

    // Determine the python packages to remove before we start installing anything new. If the
    // python version changed between installations we will have to remove any previous distribution
    // regardless.
    let (python_packages_to_remove, python_packages_to_install) =
        determine_python_packages_to_remove_and_install(
            transaction.current_python_info.as_ref(),
            transaction.python_info.as_ref(),
            current_python_distributions,
            python_packages,
        );

    // Remove python packages that need to be removed
    if !python_packages_to_remove.is_empty() {
        let python_version = transaction.current_python_info.as_ref().map(|p| (p.short_version.0 as u32, p.short_version.1 as u32)).expect(
            "there cannot be any installed python package without a previous python installation",
        );

        // Get the site_package path since everything is relative to that directory.
        let install_paths = InstallPaths::for_venv(python_version, platform.is_windows());
        let site_package_path = install_paths
            .site_packages()
            .expect("site-packages path must exist");

        // Remove the python packages
        for python_package in python_packages_to_remove {
            tracing::info!(
                "uninstalling python package {}-{}",
                &python_package.name,
                &python_package.version
            );
            let relative_dist_info = python_package
                .dist_info
                .strip_prefix(site_package_path)
                .expect("the dist-info path must be a sub-path of the site-packages path");
            rip::uninstall::uninstall_distribution(&prefix.root().join(site_package_path), relative_dist_info)
                .into_diagnostic()
                .with_context(|| format!("could not uninstall python package {}-{}. Manually remove the `.pixi/env` folder and try again.", &python_package.name, &python_package.version))?;
        }
    }

    // Execute the transaction if there is work to do
    if !transaction.operations.is_empty() {
        // Execute the operations that are returned by the solver.
        await_in_progress(
            "updating environment",
            install::execute_transaction(
                transaction,
                prefix.root().to_path_buf(),
                rattler::default_cache_dir()
                    .map_err(|_| miette::miette!("could not determine default cache directory"))?,
                default_authenticated_client(),
            ),
        )
        .await?;
    }

    // Get the pip packages to install
    if !python_packages_to_install.is_empty() {
        let python_version =
            python_version.ok_or_else(|| miette::miette!("no python version in transaction"))?;
        let install_paths = InstallPaths::for_venv(python_version, platform.is_windows());

        for package in python_packages_to_install {
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
            tracing::info!("installing python package {filename}",);

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
            wheel
                .unpack(
                    prefix.root(),
                    &install_paths,
                    &UnpackWheelOptions {
                        installer: Some(env!("CARGO_PKG_NAME").into()),
                    },
                )
                .into_diagnostic()?;
        }
    }

    Ok(())
}

/// Determine which python packages we can leave untouched and which python packages should be
/// removed.
fn determine_python_packages_to_remove_and_install<'p>(
    current_python_info: Option<&PythonInfo>,
    desired_python_info: Option<&PythonInfo>,
    mut current_python_packages: Vec<Distribution>,
    desired_python_packages: Vec<&'p LockedDependency>,
) -> (Vec<Distribution>, Vec<&'p LockedDependency>) {
    // If the python version do not match we have to remove all python packages and reinstall.
    if current_python_info.map(|p| p.short_version) != desired_python_info.map(|p| p.short_version)
    {
        return (current_python_packages, desired_python_packages);
    }

    // Determine the artifact tags associated with the locked dependencies.
    let mut desired_python_packages = extract_locked_tags(desired_python_packages);

    // Remove any package that we also have as a locked dependency. If an installed package matches
    // a locked package we can assume that it has already been installed.
    current_python_packages.retain(|current_python_packages| {
        if let Some(found_desired_packages_idx) =
            desired_python_packages
                .iter()
                .position(|(pkg, artifact_name)| {
                    does_installed_match_locked_package(
                        &current_python_packages,
                        (&pkg, artifact_name.as_ref()),
                    )
                })
        {
            // Remove from the desired list of packages to install & from the packages to uninstall.
            desired_python_packages.remove(found_desired_packages_idx);
            false
        } else {
            true
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

/// Returns the python distributions installed in the given prefix that have not been installed as
/// conda packages.
fn find_externally_installed_python_distributions(
    prefix: &Prefix,
    platform: Platform,
    py_info: &PythonInfo,
) -> Result<Vec<Distribution>, FindDistributionError> {
    // Determine where packages would have been installed
    let current_install_paths = InstallPaths::for_venv(
        (
            py_info.short_version.0 as u32,
            py_info.short_version.1 as u32,
        ),
        platform.is_windows(),
    );

    // Determine the current python distributions in those locations
    let mut current_python_packages =
        rip::find_distributions_in_venv(prefix.root(), &current_install_paths)?;

    // Remove any packages that have been installed as conda packages. Python conda packages
    // have their INSTALLER conveniently set to "conda".
    current_python_packages.retain(|d| d.installer.as_deref() != Some("conda"));

    Ok(current_python_packages)
}
