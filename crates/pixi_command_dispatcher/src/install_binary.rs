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

use crate::compute_data::{HasAllowExecuteLinkScripts, HasDownloadClient, HasPackageCache};
use crate::install_pixi::reporter::WrappingInstallReporter;

/// Install the given binary records into `prefix`.
///
/// This is a thin wrapper around [`rattler::install::Installer`] that
/// reads its resources from the compute engine's [`DataStore`]. It
/// assumes the caller has already obtained whatever cross-process lock
/// it needs on the prefix.
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
    reporter: Option<Box<dyn rattler::install::Reporter>>,
) -> Result<(), InstallerError> {
    let mut installer = Installer::new()
        .with_target_platform(target_platform)
        .with_download_client(data.download_client().clone())
        .with_package_cache(data.package_cache().clone())
        .with_execute_link_scripts(data.allow_execute_link_scripts());

    if let Some(reporter) = reporter {
        installer = installer.with_reporter(WrappingInstallReporter(reporter));
    }

    installer.install(prefix.path(), records).await.map(|_| ())
}
