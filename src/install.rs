use crate::default_retry_policy;
use crate::progress::{
    default_progress_style, finished_progress_style, global_multi_progress,
    ProgressBarMessageFormatter,
};
use futures::future::ready;
use futures::{stream, FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use itertools::Itertools;
use miette::{IntoDiagnostic, WrapErr};
use rattler::install::{
    link_package, InstallDriver, InstallOptions, Transaction, TransactionOperation,
};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{PrefixRecord, RepoDataRecord};
use rattler_networking::AuthenticatedClient;
use std::cmp::Ordering;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Executes the transaction on the given environment.
pub async fn execute_transaction(
    transaction: &Transaction<PrefixRecord, RepoDataRecord>,
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
        Some(ProgressBarMessageFormatter::new(pb))
    } else {
        None
    };

    // Create a progress bar to track all operations.
    let total_operations = transaction.operations.len();
    let link_pb = {
        let pb = multi_progress.add(
            indicatif::ProgressBar::new(total_operations as u64)
                .with_style(default_progress_style())
                .with_finish(indicatif::ProgressFinish::WithMessage("Done!".into()))
                .with_prefix("linking"),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        ProgressBarMessageFormatter::new(pb)
    };

    // Sort the operations to try to optimize the installation time.
    let sorted_operations = transaction
        .operations
        .iter()
        .enumerate()
        .sorted_unstable_by(|&(a_idx, a), &(b_idx, b)| {
            // Sort the operations so we first install packages and then remove them. We do it in
            // this order because downloading takes time so we want to do that as soon as possible
            match (a.record_to_install(), b.record_to_install()) {
                (Some(a), Some(b)) => {
                    // If we have two packages sort them by size, the biggest goes first.
                    let a_size = a.package_record.size.or(a.package_record.legacy_bz2_size);
                    let b_size = b.package_record.size.or(b.package_record.legacy_bz2_size);
                    if let (Some(a_size), Some(b_size)) = (a_size, b_size) {
                        match a_size.cmp(&b_size) {
                            Ordering::Less => return Ordering::Greater,
                            Ordering::Greater => return Ordering::Less,
                            Ordering::Equal => {}
                        }
                    }
                }
                (Some(_), None) => {
                    return Ordering::Less;
                }
                (None, Some(_)) => {
                    return Ordering::Greater;
                }
                _ => {}
            }

            // Otherwise keep the original order as much as possible.
            a_idx.cmp(&b_idx)
        })
        .map(|(_, op)| op);

    // Perform all transactions operations in parallel.
    let result = stream::iter(sorted_operations.into_iter())
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
        download_pb.into_progress_bar().finish_and_clear();
    }
    link_pb.into_progress_bar().finish_and_clear();

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
    download_pb: Option<&ProgressBarMessageFormatter>,
    link_pb: &ProgressBarMessageFormatter,
    op: &TransactionOperation<PrefixRecord, RepoDataRecord>,
    install_options: &InstallOptions,
) -> miette::Result<()> {
    // Determine the package to install
    let install_record = op.record_to_install();
    let remove_record = op.record_to_remove();

    // Create a future to remove the existing package
    let remove_future = if let Some(remove_record) = remove_record {
        link_pb
            .wrap(
                format!(
                    "removing {} {}",
                    &remove_record
                        .repodata_record
                        .package_record
                        .name
                        .as_source(),
                    &remove_record.repodata_record.package_record.version
                ),
                remove_package_from_environment(target_prefix, remove_record),
            )
            .left_future()
    } else {
        ready(Ok(())).right_future()
    };

    // Create a future to download the package
    let cached_package_dir_fut = if let Some(install_record) = install_record {
        async {
            let task = if let Some(pb) = download_pb {
                Some(
                    pb.start(install_record.package_record.name.as_source().to_string())
                        .await,
                )
            } else {
                None
            };

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
            if let Some(task) = task {
                let pb = task.finish().await;
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
        link_pb
            .wrap(
                record.package_record.name.as_source().to_string(),
                install_package_to_environment(
                    target_prefix,
                    package_dir,
                    record.clone(),
                    install_driver,
                    install_options,
                ),
            )
            .await?;
    }

    // Increment the link progress bar since we finished a step!
    link_pb.progress_bar().inc(1);
    if link_pb.progress_bar().length() == Some(link_pb.progress_bar().position()) {
        link_pb.progress_bar().set_style(finished_progress_style());
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
