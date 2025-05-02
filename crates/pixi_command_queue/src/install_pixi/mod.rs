use crate::{CommandQueue, CommandQueueError};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_record::PixiRecord;
use rattler::install::{Installer, InstallerError};
use rattler_conda_types::prefix::Prefix;
use rattler_conda_types::{Platform, PrefixRecord};
use std::collections::HashSet;
use thiserror::Error;

pub struct InstallPixiEnvironmentSpec {
    /// The specification of the environment to install.
    pub records: Vec<PixiRecord>,

    /// The location to create the prefix at.
    pub prefix: Prefix,

    /// If already known, the installed packages
    pub installed: Option<Vec<PrefixRecord>>,

    /// The platform for which the environment is installed.
    pub platform: Platform,

    /// Packages to force reinstalling.
    pub force_reinstall: HashSet<rattler_conda_types::PackageName>,
}

impl InstallPixiEnvironmentSpec {
    pub async fn install(
        self,
        command_queue: CommandQueue,
    ) -> Result<(), CommandQueueError<InstallPixiEnvironmentError>> {
        // Split into source and binary records
        let (source_records, binary_records): (Vec<_>, Vec<_>) = self
            .records
            .into_iter()
            .partition_map(|record| match record {
                PixiRecord::Source(record) => Either::Left(record),
                PixiRecord::Binary(record) => Either::Right(record),
            });

        assert!(
            source_records.is_empty(),
            "TODO installation of source records is not yet implemented"
        );

        let mut installer = Installer::new()
            .with_target_platform(self.platform)
            .with_download_client(command_queue.download_client().clone())
            .with_package_cache(command_queue.package_cache().clone())
            .with_reinstall_packages(self.force_reinstall);

        if let Some(installed) = self.installed {
            installer = installer.with_installed_packages(installed);
        };

        let _result = installer
            .install(self.prefix.path(), binary_records)
            .await
            .map_err(InstallPixiEnvironmentError::Installer)?;

        Ok(())
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstallPixiEnvironmentError {
    #[error(transparent)]
    Installer(InstallerError),
}
