//! Free function that installs a set of binary `RepoDataRecord`s into a
//! prefix. Used by compute-engine Keys that manage disposable / workspace
//! environments.
//!
//! This is intentionally a thin wrapper around `rattler::install::Installer`
//! that reads its resources (download client, package cache, link-script
//! policy) from the compute engine's [`DataStore`] rather than a
//! `CommandDispatcher`.

use pixi_compute_engine::DataStore;
use rattler::install::{Installer, InstallerError};
use rattler_conda_types::{Platform, RepoDataRecord, prefix::Prefix};

use crate::compute_data::{
    HasAllowExecuteLinkScripts, HasAllowLinkOptions, HasIoConcurrencySemaphore, HasPackageCache,
};
use crate::install_pixi::reporter::WrappingInstallReporter;
use pixi_compute_network::HasDownloadClient;

/// Install the given binary records into `prefix`.
///
/// This is a thin wrapper around [`rattler::install::Installer`] that
/// reads its resources from the compute engine's [`DataStore`]. It
/// assumes the caller has already obtained whatever cross-process lock
/// it needs on the prefix.
///
/// When `reinstall_all` is set every record is re-linked even if
/// conda-meta claims it is already present; use this to recover a
/// prefix left dirty by an interrupted install.
///
/// The installer's structured result is not returned because
/// `InstallationResult` is not exported from the `rattler::install`
/// module. Callers that need the transaction / link-script details
/// should call [`CommandDispatcher::install_pixi_environment`](crate::CommandDispatcher::install_pixi_environment)
/// instead.
pub async fn install_binary_records(
    data: &DataStore,
    prefix: &Prefix,
    records: Vec<RepoDataRecord>,
    target_platform: Platform,
    reinstall_all: bool,
    reporter: Option<Box<dyn rattler::install::Reporter>>,
) -> Result<(), InstallerError> {
    let mut installer = Installer::new()
        .with_target_platform(target_platform)
        .with_download_client(data.download_client().clone())
        .with_package_cache(data.package_cache().clone())
        .with_execute_link_scripts(data.allow_execute_link_scripts())
        .with_link_options(data.allow_link_options());

    if let Some(io_semaphore) = data.io_concurrency_semaphore() {
        installer = installer.with_io_concurrency_semaphore(io_semaphore.clone());
    }

    if reinstall_all {
        installer = installer.with_reinstall_packages(
            records
                .iter()
                .map(|r| r.package_record.name.clone())
                .collect(),
        );
    }

    if let Some(reporter) = reporter {
        installer = installer.with_reporter(WrappingInstallReporter(reporter));
    }

    installer.install(prefix.path(), records).await.map(|_| ())
}
