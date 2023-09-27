use crate::{
    consts, default_retry_policy,
    prefix::Prefix,
    progress::{
        await_in_progress, default_progress_style, finished_progress_style, global_multi_progress,
    },
    virtual_packages::verify_current_platform_has_required_virtual_packages,
    Project,
};
use futures::future::ready;
use futures::{stream, FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic, LabeledSpan};
use rattler::{
    install::{link_package, InstallDriver, InstallOptions, Transaction, TransactionOperation},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    conda_lock,
    conda_lock::builder::{LockFileBuilder, LockedPackage, LockedPackages},
    conda_lock::CondaLock,
    MatchSpec, NamelessMatchSpec, PackageName, Platform, PrefixRecord, RepoDataRecord, Version,
};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{libsolv_rs, SolverImpl};
use std::collections::HashMap;
use std::{
    collections::{HashSet, VecDeque},
    io::ErrorKind,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

/// Returns the prefix associated with the given environment. If the prefix doesn't exist or is not
/// up to date it is updated.
pub async fn get_up_to_date_prefix(project: &Project) -> miette::Result<Prefix> {
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
    let prefix = Prefix::new(project.root().join(consts::PIXI_FOLDER_PATH).join("env"))?;
    let installed_packages_future = {
        let prefix = prefix.clone();
        tokio::spawn(async move { prefix.find_installed_packages(None).await })
    };

    // Update the lock-file if it is out of date.
    let lock_file = load_lock_file(project).await?;
    let lock_file = if !lock_file_up_to_date(project, &lock_file)? {
        update_lock_file(project, lock_file, None).await?
    } else {
        lock_file
    };

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
        get_required_packages(lock_file, platform)?,
        platform,
    )
    .into_diagnostic()?;

    // Execute the transaction if there is work to do
    if !transaction.operations.is_empty() {
        // Execute the operations that are returned by the solver.
        await_in_progress(
            "updating environment",
            execute_transaction(
                transaction,
                prefix.root().to_path_buf(),
                rattler::default_cache_dir()
                    .map_err(|_| miette::miette!("could not determine default cache directory"))?,
                AuthenticatedClient::default(),
            ),
        )
        .await?;
    }
    Ok(())
}

/// Loads the lockfile for the specified project or returns a dummy one if none could be found.
pub async fn load_lock_for_manifest_path(path: &Path) -> miette::Result<CondaLock> {
    load_lock_file(&Project::load(path)?).await
}

/// Loads the lockfile for the specified project or returns a dummy one if none could be found.
pub async fn load_lock_file(project: &Project) -> miette::Result<CondaLock> {
    let lock_file_path = project.lock_file_path();
    tokio::task::spawn_blocking(move || {
        if lock_file_path.is_file() {
            CondaLock::from_path(&lock_file_path).into_diagnostic()
        } else {
            LockFileBuilder::default().build().into_diagnostic()
        }
    })
    .await
    .unwrap_or_else(|e| Err(e).into_diagnostic())
}

/// Returns true if the locked packages match the dependencies in the project.
pub fn lock_file_up_to_date(project: &Project, lock_file: &CondaLock) -> miette::Result<bool> {
    let platforms = project.platforms();

    // If a platform is missing from the lock file the lock file is completely out-of-date.
    if HashSet::<Platform>::from_iter(lock_file.metadata.platforms.iter().copied())
        != HashSet::from_iter(platforms.iter().copied())
    {
        return Ok(false);
    }

    // Check if the channels in the lock file match our current configuration. Note that the order
    // matters here. If channels are added in a different order, the solver might return a different
    // result.
    let channels = project
        .channels()
        .iter()
        .map(|channel| conda_lock::Channel::from(channel.base_url().to_string()))
        .collect_vec();
    if lock_file.metadata.channels.iter().ne(channels.iter()) {
        return Ok(false);
    }

    // For each platform,
    for platform in platforms.iter().cloned() {
        // Check if all dependencies exist in the lock-file.
        let dependencies = project
            .all_dependencies(platform)?
            .into_iter()
            .collect::<VecDeque<_>>();

        // Construct a queue of dependencies that we wanna find in the lock file
        let mut queue = dependencies.clone();

        // Get the virtual packages for the system
        let virtual_packages = project
            .virtual_packages(platform)?
            .into_iter()
            .map(|vpkg| (vpkg.name.clone(), vpkg))
            .collect::<HashMap<_, _>>();

        // Keep track of which dependencies we already found. Since there can always only be one
        // version per named package we can just keep track of the package names.
        let mut seen = dependencies
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<HashSet<_>>();

        while let Some((name, spec)) = queue.pop_back() {
            // Is this a virtual package? And does it match?
            if let Some(vpkg) = virtual_packages.get(&name) {
                if let Some(version_spec) = spec.version {
                    if !version_spec.matches(&vpkg.version) {
                        tracing::info!("found a dependency on virtual package '{}' but the version spec '{}' does not match the expected version of the virtual package '{}'.", name.as_source(), &version_spec, &vpkg.version);
                        return Ok(false);
                    }
                }
                if let Some(build_spec) = spec.build {
                    if !build_spec.matches(&vpkg.build_string) {
                        tracing::info!("found a dependency on virtual package '{}' but the build spec '{}' does not match the expected build of the virtual package '{}'.", name.as_source(), &build_spec, &vpkg.build_string);
                        return Ok(false);
                    }
                }

                // Virtual package matches
                continue;
            }

            // Find the package in the lock-file that matches our dependency.
            let locked_package = lock_file
                .packages_for_platform(platform)
                .find(|locked_package| locked_dependency_satisfies(locked_package, &name, &spec));

            match locked_package {
                None => {
                    // No package found that matches the dependency, the lock file is not in a
                    // consistent state.
                    tracing::info!("failed to find a locked package for '{} {}', assuming the lock file is out of date.", name.as_source(), &spec);
                    return Ok(false);
                }
                Some(package) => {
                    for (depends_name, depends_constraint) in package.dependencies.iter() {
                        if !seen.contains(depends_name) {
                            // Parse the constraint
                            match NamelessMatchSpec::from_str(&depends_constraint.to_string()) {
                                Ok(spec) => {
                                    queue.push_back((depends_name.clone(), spec));
                                    seen.insert(depends_name.clone());
                                }
                                Err(_) => {
                                    tracing::warn!("failed to parse spec '{}', assuming the lock file is corrupt.", depends_constraint);
                                    return Ok(false);
                                }
                            }
                        }
                    }
                }
            }
        }

        // If the number of "seen" dependencies is less than the number of packages for this
        // platform in the first place, there are more packages in the lock file than are used. This
        // means the lock file is also out of date.
        if seen.len() < lock_file.packages_for_platform(platform).count() {
            tracing::info!("there are more packages in the lock-file than required to fulfill all dependency requirements. Assuming the lock file is out of date.");
            return Ok(false);
        }
    }

    Ok(true)
}

/// Returns true if the specified [`conda_lock::LockedDependency`] satisfies the given MatchSpec.
/// TODO: Move this back to rattler.
/// TODO: Make this more elaborate to include all properties of MatchSpec
fn locked_dependency_satisfies(
    locked_package: &conda_lock::LockedDependency,
    name: &PackageName,
    spec: &NamelessMatchSpec,
) -> bool {
    // Check if the name of the package matches
    if locked_package.name != *name {
        return false;
    }

    // Check if the version matches
    if let Some(version_spec) = &spec.version {
        let v = match Version::from_str(&locked_package.version) {
            Err(_) => return false,
            Ok(v) => v,
        };

        if !version_spec.matches(&v) {
            return false;
        }
    }

    // Check if the build string matches
    match (spec.build.as_ref(), &locked_package.build) {
        (Some(build_spec), Some(build)) => {
            if !build_spec.matches(build) {
                return false;
            }
        }
        (Some(_), None) => return false,
        _ => {}
    }

    true
}

/// Updates the lock file for a project.
pub async fn update_lock_file(
    project: &Project,
    existing_lock_file: CondaLock,
    repodata: Option<Vec<SparseRepoData>>,
) -> miette::Result<CondaLock> {
    let platforms = project.platforms();

    // Get the repodata for the project
    let sparse_repo_data = if let Some(sparse_repo_data) = repodata {
        sparse_repo_data
    } else {
        project.fetch_sparse_repodata().await?
    };

    // Construct a conda lock file
    let channels = project
        .channels()
        .iter()
        .map(|channel| conda_lock::Channel::from(channel.base_url().to_string()));

    // Empty match-specs because these differ per platform
    let mut builder = LockFileBuilder::new(channels, platforms.iter().cloned(), vec![]);
    for platform in platforms.iter().cloned() {
        let dependencies = project.all_dependencies(platform)?;
        let match_specs = dependencies
            .iter()
            .map(|(name, constraint)| {
                MatchSpec::from_nameless(constraint.clone(), Some(name.clone()))
            })
            .collect_vec();

        // Extract the package names from the dependencies
        let package_names = dependencies.keys().collect_vec();

        // Get the repodata for the current platform and for NoArch
        let platform_sparse_repo_data = sparse_repo_data.iter().filter(|sparse| {
            sparse.subdir() == platform.as_str() || sparse.subdir() == Platform::NoArch.as_str()
        });

        // Load only records we need for this platform
        let available_packages = SparseRepoData::load_records_recursive(
            platform_sparse_repo_data,
            package_names.into_iter().cloned(),
            None,
        )
        .into_diagnostic()?;

        // Get the virtual packages for this platform
        let virtual_packages = project.virtual_packages(platform)?;

        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs: match_specs.clone(),
            available_packages: &available_packages,
            locked_packages: existing_lock_file
                .packages_for_platform(platform)
                .map(RepoDataRecord::try_from)
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?,
            pinned_packages: vec![],
            virtual_packages,
        };

        // Solve the task
        let records = libsolv_rs::Solver.solve(task).into_diagnostic()?;

        // Update lock file
        let mut locked_packages = LockedPackages::new(platform);
        for record in records {
            let locked_package = LockedPackage::try_from(record).into_diagnostic()?;
            locked_packages = locked_packages.add_locked_package(locked_package);
        }

        builder = builder.add_locked_packages(locked_packages);
    }

    let conda_lock = builder.build().into_diagnostic()?;

    // Write the conda lock to disk
    conda_lock
        .to_path(&project.lock_file_path())
        .into_diagnostic()?;

    Ok(conda_lock)
}

/// Returns the [`RepoDataRecord`]s for the packages of the current platform from the lock-file.
pub fn get_required_packages(
    lock_file: &CondaLock,
    platform: Platform,
) -> miette::Result<Vec<RepoDataRecord>> {
    lock_file
        .package
        .iter()
        .filter(|pkg| pkg.platform == platform)
        .map(|pkg| pkg.clone().try_into().into_diagnostic())
        .collect()
}

/// Executes the transaction on the given environment.
pub async fn execute_transaction(
    transaction: Transaction<PrefixRecord, RepoDataRecord>,
    target_prefix: PathBuf,
    cache_dir: PathBuf,
    download_client: AuthenticatedClient,
) -> miette::Result<()> {
    // Open the package cache
    let package_cache = PackageCache::new(cache_dir.join("pkgs"));

    // Create an install driver which helps limit the number of concurrent filesystem operations
    let install_driver = InstallDriver::default();

    // Define default installation options.
    let install_options = InstallOptions {
        python_info: transaction.python_info.clone(),
        platform: Some(transaction.platform),
        ..Default::default()
    };

    // Create a progress bars for downloads.
    let multi_progress = global_multi_progress();
    let total_packages_to_download = transaction
        .operations
        .iter()
        .filter(|op| op.record_to_install().is_some())
        .count();
    let download_pb = if total_packages_to_download > 0 {
        let pb = multi_progress.add(
            indicatif::ProgressBar::new(total_packages_to_download as u64)
                .with_style(default_progress_style())
                .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
                .with_prefix("downloading"),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        Some(pb)
    } else {
        None
    };

    // Create a progress bar to track all operations.
    let total_operations = transaction.operations.len();
    let link_pb = multi_progress.add(
        indicatif::ProgressBar::new(total_operations as u64)
            .with_style(default_progress_style())
            .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
            .with_prefix("linking"),
    );
    link_pb.enable_steady_tick(Duration::from_millis(100));

    // Perform all transactions operations in parallel.
    let result = stream::iter(transaction.operations)
        .map(Ok)
        .try_for_each_concurrent(50, |op| {
            let target_prefix = target_prefix.clone();
            let download_client = download_client.clone();
            let package_cache = &package_cache;
            let install_driver = &install_driver;
            let download_pb = download_pb.as_ref();
            let link_pb = &link_pb;
            let install_options = &install_options;
            async move {
                execute_operation(
                    &target_prefix,
                    download_client,
                    package_cache,
                    install_driver,
                    download_pb,
                    link_pb,
                    op,
                    install_options,
                )
                .await
            }
        })
        .await;

    // Clear progress bars
    if let Some(download_pb) = download_pb {
        download_pb.finish_and_clear();
    }
    link_pb.finish_and_clear();

    result
}

/// Executes a single operation of a transaction on the environment.
/// TODO: Move this into an object or something.
#[allow(clippy::too_many_arguments)]
async fn execute_operation(
    target_prefix: &Path,
    download_client: AuthenticatedClient,
    package_cache: &PackageCache,
    install_driver: &InstallDriver,
    download_pb: Option<&ProgressBar>,
    link_pb: &ProgressBar,
    op: TransactionOperation<PrefixRecord, RepoDataRecord>,
    install_options: &InstallOptions,
) -> miette::Result<()> {
    // Determine the package to install
    let install_record = op.record_to_install();
    let remove_record = op.record_to_remove();

    // Create a future to remove the existing package
    let remove_future = if let Some(remove_record) = remove_record {
        remove_package_from_environment(target_prefix, remove_record).left_future()
    } else {
        ready(Ok(())).right_future()
    };

    // Create a future to download the package
    let cached_package_dir_fut = if let Some(install_record) = install_record {
        async {
            // Make sure the package is available in the package cache.
            let result = package_cache
                .get_or_fetch_from_url_with_retry(
                    &install_record.package_record,
                    install_record.url.clone(),
                    download_client.clone(),
                    default_retry_policy(),
                )
                .map_ok(|cache_dir| Some((install_record.clone(), cache_dir)))
                .await
                .into_diagnostic();

            // Increment the download progress bar.
            if let Some(pb) = download_pb {
                pb.inc(1);
                if pb.length() == Some(pb.position()) {
                    pb.set_style(finished_progress_style());
                }
            }

            result
        }
        .left_future()
    } else {
        ready(Ok(None)).right_future()
    };

    // Await removal and downloading concurrently
    let (_, install_package) = tokio::try_join!(remove_future, cached_package_dir_fut)?;

    // If there is a package to install, do that now.
    if let Some((record, package_dir)) = install_package {
        install_package_to_environment(
            target_prefix,
            package_dir,
            record.clone(),
            install_driver,
            install_options,
        )
        .await?;
    }

    // Increment the link progress bar since we finished a step!
    link_pb.inc(1);
    if link_pb.length() == Some(link_pb.position()) {
        link_pb.set_style(finished_progress_style());
    }

    Ok(())
}

/// Install a package into the environment and write a `conda-meta` file that contains information
/// about how the file was linked.
async fn install_package_to_environment(
    target_prefix: &Path,
    package_dir: PathBuf,
    repodata_record: RepoDataRecord,
    install_driver: &InstallDriver,
    install_options: &InstallOptions,
) -> miette::Result<()> {
    // Link the contents of the package into our environment. This returns all the paths that were
    // linked.
    let paths = link_package(
        &package_dir,
        target_prefix,
        install_driver,
        install_options.clone(),
    )
    .await
    .into_diagnostic()?;

    // Construct a PrefixRecord for the package
    let prefix_record = PrefixRecord {
        repodata_record,
        package_tarball_full_path: None,
        extracted_package_dir: Some(package_dir),
        files: paths
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect(),
        paths_data: paths.into(),
        // TODO: Retrieve the requested spec for this package from the request
        requested_spec: None,
        // TODO: What to do with this?
        link: None,
    };

    // Create the conda-meta directory if it doesn't exist yet.
    let target_prefix = target_prefix.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path)?;

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(format!(
            "{}-{}-{}.json",
            prefix_record
                .repodata_record
                .package_record
                .name
                .as_source(),
            prefix_record.repodata_record.package_record.version,
            prefix_record.repodata_record.package_record.build
        ));
        prefix_record.write_to_path(pkg_meta_path, true)
    })
    .await
    {
        Ok(result) => Ok(result.into_diagnostic()?),
        Err(err) => {
            if let Ok(panic) = err.try_into_panic() {
                std::panic::resume_unwind(panic);
            }
            // The operation has been cancelled, so we can also just ignore everything.
            Ok(())
        }
    }
}

/// Completely remove the specified package from the environment.
async fn remove_package_from_environment(
    target_prefix: &Path,
    package: &PrefixRecord,
) -> miette::Result<()> {
    // TODO: Take into account any clobbered files, they need to be restored.
    // TODO: Can we also delete empty directories?

    // Remove all entries
    for paths in package.paths_data.paths.iter() {
        match tokio::fs::remove_file(target_prefix.join(&paths.relative_path)).await {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // Simply ignore if the file is already gone.
            }
            Err(e) => {
                return Err(e).into_diagnostic().wrap_err(format!(
                    "failed to delete {}",
                    paths.relative_path.display()
                ))
            }
        }
    }

    // Remove the conda-meta file
    let conda_meta_path = target_prefix.join("conda-meta").join(format!(
        "{}-{}-{}.json",
        package.repodata_record.package_record.name.as_normalized(),
        package.repodata_record.package_record.version,
        package.repodata_record.package_record.build
    ));
    tokio::fs::remove_file(conda_meta_path)
        .await
        .into_diagnostic()?;

    Ok(())
}
