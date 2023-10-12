use crate::{
    default_authenticated_client, install, lock_file, prefix::Prefix, progress::await_in_progress,
    virtual_packages::verify_current_platform_has_required_virtual_packages, Project,
};
use miette::{IntoDiagnostic, LabeledSpan};
use rattler::install::Transaction;
use rattler_conda_types::{Platform, PrefixRecord};
use rattler_lock::CondaLock;
use rip::{ArtifactHashes, ArtifactInfo, ArtifactName, PackageDb, Wheel, WheelName};
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
    let pip_packages = lock_file
        .get_packages_by_platform(platform)
        .filter(|pkg| pkg.is_pip());
    for package in pip_packages {
        let pip_package = package
            .as_pip()
            .expect("must be a pip package at this point");

        // TODO: Kind of a hack but get the filename from the url
        let filename = pip_package
            .url
            .path_segments()
            .and_then(|s| s.last())
            .expect("url is missing a path");
        let filename =
            WheelName::from_str(filename).expect("failed to convert filename to wheel filename");

        // Reconstruct the ArtifactInfo from the data in the lockfile.
        let artifact_info = ArtifactInfo {
            filename: ArtifactName::Wheel(filename),
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

        tracing::warn!("fetching {}", &artifact_info.filename);
        let wheel: Wheel = package_db.get_artifact(&artifact_info).await?;

    }

    Ok(())
}
