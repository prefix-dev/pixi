use crate::{
    default_authenticated_client, install, lock_file, prefix::Prefix, progress::await_in_progress,
    virtual_packages::verify_current_platform_has_required_virtual_packages, Project,
};
use miette::{IntoDiagnostic, LabeledSpan};
use rattler::install::Transaction;
use rattler_conda_types::{conda_lock::CondaLock, Platform, PrefixRecord};

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
    prefix: &Prefix,
    installed_packages: Vec<PrefixRecord>,
    lock_file: &CondaLock,
    platform: Platform,
) -> miette::Result<()> {
    // Construct a transaction to bring the environment up to date with the lock-file content
    let transaction = Transaction::from_current_and_desired(
        installed_packages,
        lock_file::get_required_packages(lock_file, platform)?,
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
    Ok(())
}
