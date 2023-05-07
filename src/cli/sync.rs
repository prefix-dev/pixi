use crate::{
    prefix::Prefix,
    progress::{default_progress_style, finished_progress_style, global_multi_progress},
    project::Project,
};
use anyhow::Context;
use clap::Parser;
use futures::{future::ready, stream, FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use rattler::{
    install::{link_package, InstallDriver, InstallOptions, Transaction, TransactionOperation},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    conda_lock::{
        self,
        builder::LockFileBuilder,
        builder::{LockedPackage, LockedPackages},
        CondaLock, PackageHashes, VersionConstraint,
    },
    ChannelConfig, MatchSpec, NamelessMatchSpec, PackageRecord, Platform, PrefixRecord,
    RepoDataRecord, Version,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{LibsolvRepoData, SolverBackend};
use reqwest::Client;
use std::{
    collections::HashSet,
    ffi::OsStr,
    io::ErrorKind,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

/// Sync the project configuration with its environment
#[derive(Parser, Debug)]
pub struct Args {}

// TODO: I dont like this command, if it is at all possible it would be so much better when this
//  command is run when needed. E.g. have a cheap way to determine if the environment is up-to-date,
//  if not, update it.
pub async fn execute(_: Args) -> anyhow::Result<()> {
    let project = Project::discover()?;
    let platforms = project.platforms()?;
    let dependencies = project.dependencies()?;

    // Load the lockfile or create a dummy one
    let lock_file_path = project.lock_file_path();
    let lock_file = if lock_file_path.is_file() {
        CondaLock::from_path(&lock_file_path)?
    } else {
        LockFileBuilder::default().build()?
    };

    // Check if the lock file is up to date with the requirements in the project.
    let specs_out_of_date = dependencies.iter().any(|(dep_name, constraints)| {
        !lock_file.package.iter().any(|locked_package| {
            locked_dependency_satisfies(locked_package, dep_name, constraints)
        })
    });
    let platforms_out_of_date =
        HashSet::<Platform>::from_iter(lock_file.metadata.platforms.iter().copied())
            != HashSet::from_iter(platforms.into_iter());
    let channels_out_of_date = false; // TODO:

    let lock_file = if platforms_out_of_date || channels_out_of_date || specs_out_of_date {
        update_lock_file(&project, lock_file).await?
    } else {
        lock_file
    };

    // Check to see if the environment is out of date or not.
    let prefix = Prefix::new(project.root().join(".pax/env"))?;
    let current_packages = prefix
        .find_installed_packages(None)
        .await
        .context("failed to determine the currently installed packages")?;

    // TODO: Stop doing anything if the currently installed packages already match our lock file
    let current_platform = Platform::current();
    let required_packages = lock_file
        .package
        .into_iter()
        .filter(|pkg| pkg.platform == current_platform)
        .map(|pkg| {
            Ok(RepoDataRecord {
                channel: String::new(),
                file_name: Path::new(pkg.url.path())
                    .file_name()
                    .and_then(OsStr::to_str)
                    .ok_or_else(|| {
                        anyhow::anyhow!("failed to determine file name from {}", &pkg.url)
                    })?
                    .to_owned(),
                url: pkg.url,
                package_record: PackageRecord {
                    arch: None,
                    build: pkg.build.unwrap_or_default(),
                    build_number: 0,
                    constrains: vec![],
                    depends: pkg
                        .dependencies
                        .into_iter()
                        .map(|(pkg_name, spec)| format!("{} {}", pkg_name, spec))
                        .collect(),
                    features: None,
                    legacy_bz2_md5: None,
                    legacy_bz2_size: None,
                    license: None,
                    license_family: None,
                    md5: match &pkg.hash {
                        PackageHashes::Md5(md5) => Some(*md5),
                        PackageHashes::Sha256(_) => None,
                        PackageHashes::Md5Sha256(md5, _) => Some(*md5),
                    },
                    name: pkg.name,
                    noarch: Default::default(),
                    platform: None,
                    sha256: match &pkg.hash {
                        PackageHashes::Md5(_) => None,
                        PackageHashes::Sha256(sha256) => Some(*sha256),
                        PackageHashes::Md5Sha256(_, sha256) => Some(*sha256),
                    },
                    size: None,
                    subdir: "".to_string(),
                    timestamp: None,
                    track_features: vec![],
                    version: Version::from_str(&pkg.version)?,
                },
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Construct a transaction to be able to update the environment to the correct versions of all
    // packages.
    let transaction = rattler::install::Transaction::from_current_and_desired(
        current_packages,
        required_packages,
        current_platform,
    )?;

    if !transaction.operations.is_empty() {
        // Execute the operations that are returned by the solver.
        execute_transaction(
            transaction,
            prefix.root().to_path_buf(),
            rattler::default_cache_dir()?,
            Client::default(),
        )
        .await?;
        println!(
            "{} Successfully updated the environment",
            console::style(console::Emoji("✔", "")).green(),
        );
    } else {
        println!(
            "{} Already up to date",
            console::style(console::Emoji("✔", "")).green(),
        );
    }

    Ok(())
}

/// Returns true if the specified [`conda_lock::LockedDependency`] satisfies the given match spec.
/// TODO: Move this back to rattler.
/// TODO: Make this more elaborate to include all properties of MatchSpec
fn locked_dependency_satisfies(
    locked_package: &conda_lock::LockedDependency,
    name: &str,
    spec: &NamelessMatchSpec,
) -> bool {
    // Check if the name of the package matches
    if locked_package.name.as_str() != name {
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

async fn update_lock_file(
    project: &Project,
    _existing_lock_file: CondaLock,
) -> anyhow::Result<CondaLock> {
    let platforms = project.platforms()?;
    let dependencies = project.dependencies()?;

    // Extract the package names from the dependencies
    let package_names = dependencies.keys().collect_vec();

    // Get the repodata for the project
    let sparse_repo_data = project.fetch_sparse_repodata().await?;

    // Construct a conda lock file
    let channels = project
        .channels(&ChannelConfig::default())?
        .into_iter()
        .map(|channel| conda_lock::Channel::from(channel.base_url().to_string()));

    let match_specs = dependencies
        .iter()
        .map(|(name, constraint)| MatchSpec::from_nameless(constraint.clone(), Some(name.clone())))
        .collect_vec();

    let mut builder = LockFileBuilder::new(channels, platforms.clone(), match_specs.clone());
    for platform in platforms {
        // Get the repodata for the current platform and for NoArch
        let platform_sparse_repo_data = sparse_repo_data.iter().filter(|sparse| {
            sparse.subdir() == platform.as_str() || sparse.subdir() == Platform::NoArch.as_str()
        });

        // Load only records we need for this platform
        let available_packages = SparseRepoData::load_records_recursive(
            platform_sparse_repo_data,
            package_names.iter().copied(),
        )?;

        // Construct a solver task that we can start solving.
        let task = rattler_solve::SolverTask {
            specs: match_specs.clone(),
            available_packages: available_packages
                .iter()
                .map(|records| LibsolvRepoData::from_records(records)),

            // TODO: All these things.
            locked_packages: vec![],
            pinned_packages: vec![],
            virtual_packages: vec![],
        };

        // Solve the task
        let records = rattler_solve::LibsolvBackend.solve(task)?;

        let mut locked_packages = LockedPackages::new(platform);
        for record in records {
            locked_packages = locked_packages.add_locked_package(LockedPackage {
                name: record.package_record.name,
                version: record.package_record.version.to_string(),
                build_string: record.package_record.build.to_string(),
                url: record.url,
                package_hashes: match (record.package_record.sha256, record.package_record.md5) {
                    (Some(sha256), Some(md5)) => PackageHashes::Md5Sha256(md5, sha256),
                    (Some(sha256), None) => PackageHashes::Sha256(sha256),
                    (None, Some(md5)) => PackageHashes::Md5(md5),
                    _ => unreachable!("package without any hash??"),
                },
                dependency_list: record
                    .package_record
                    .depends
                    .iter()
                    .map(|dep| {
                        MatchSpec::from_str(dep)
                            .map_err(anyhow::Error::from)
                            .and_then(|spec| match &spec.name {
                                Some(name) => Ok((
                                    name.to_owned(),
                                    VersionConstraint::from(NamelessMatchSpec::from(spec)),
                                )),
                                None => Err(anyhow::anyhow!(
                                    "dependency matchspec missing a name '{}'",
                                    dep
                                )),
                            })
                    })
                    .collect::<Result<_, _>>()?,
                optional: None,
            });
        }

        builder = builder.add_locked_packages(locked_packages);
    }

    let conda_lock = builder.build()?;

    // Write the conda lock to disk
    conda_lock.to_path(&project.lock_file_path())?;

    Ok(conda_lock)
}

/// Executes the transaction on the given environment.
async fn execute_transaction(
    transaction: Transaction<PrefixRecord, RepoDataRecord>,
    target_prefix: PathBuf,
    cache_dir: PathBuf,
    download_client: Client,
) -> anyhow::Result<()> {
    // Open the package cache
    let package_cache = PackageCache::new(cache_dir.join("pkgs"));

    // Create an install driver which helps limit the number of concurrent fileystem operations
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
    stream::iter(transaction.operations)
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
        .await?;

    Ok(())
}

/// Executes a single operation of a transaction on the environment.
/// TODO: Move this into an object or something.
#[allow(clippy::too_many_arguments)]
async fn execute_operation(
    target_prefix: &Path,
    download_client: Client,
    package_cache: &PackageCache,
    install_driver: &InstallDriver,
    download_pb: Option<&ProgressBar>,
    link_pb: &ProgressBar,
    op: TransactionOperation<PrefixRecord, RepoDataRecord>,
    install_options: &InstallOptions,
) -> anyhow::Result<()> {
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
                .get_or_fetch_from_url(
                    &install_record.package_record,
                    install_record.url.clone(),
                    download_client.clone(),
                )
                .map_ok(|cache_dir| Some((install_record.clone(), cache_dir)))
                .map_err(anyhow::Error::from)
                .await;

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
) -> anyhow::Result<()> {
    // Link the contents of the package into our environment. This returns all the paths that were
    // linked.
    let paths = link_package(
        &package_dir,
        target_prefix,
        install_driver,
        install_options.clone(),
    )
    .await?;

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

    // Create the conda-meta directory if it doesnt exist yet.
    let target_prefix = target_prefix.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path)?;

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(format!(
            "{}-{}-{}.json",
            prefix_record.repodata_record.package_record.name,
            prefix_record.repodata_record.package_record.version,
            prefix_record.repodata_record.package_record.build
        ));
        prefix_record.write_to_path(pkg_meta_path, true)
    })
    .await
    {
        Ok(result) => Ok(result?),
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
) -> anyhow::Result<()> {
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
                return Err(e)
                    .with_context(|| format!("failed to delete {}", paths.relative_path.display()))
            }
        }
    }

    // Remove the conda-meta file
    let conda_meta_path = target_prefix.join("conda-meta").join(format!(
        "{}-{}-{}.json",
        package.repodata_record.package_record.name,
        package.repodata_record.package_record.version,
        package.repodata_record.package_record.build
    ));
    tokio::fs::remove_file(conda_meta_path).await?;

    Ok(())
}
