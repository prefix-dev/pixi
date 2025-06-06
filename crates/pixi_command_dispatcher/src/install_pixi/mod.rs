use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    path::Path,
};

use chrono::Utc;
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::PlatformAndVirtualPackages;
use pixi_record::PixiRecord;
use rattler::install::{Installer, InstallerError};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, Platform, PrefixRecord, RepoDataRecord, prefix::Prefix,
};
use rattler_digest::Sha256Hash;
use thiserror::Error;
use url::Url;

use crate::{
    CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    SourceBuildError, SourceBuildSpec,
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPixiEnvironmentSpec {
    /// The specification of the environment to install.
    #[serde(skip)]
    pub records: Vec<PixiRecord>,

    /// The location to create the prefix at.
    #[serde(skip)]
    pub prefix: Prefix,

    /// If already known, the installed packages
    #[serde(skip)]
    pub installed: Option<Vec<PrefixRecord>>,

    /// Describes the platform
    pub target_platform: Platform,

    /// Packages to force reinstalling.
    #[serde(skip_serializing_if = "HashSet::is_empty")]
    pub force_reinstall: HashSet<rattler_conda_types::PackageName>,

    /// The channels to use when building source packages.
    pub channels: Vec<ChannelUrl>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,

    /// Build variants to use during the solve
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

impl InstallPixiEnvironmentSpec {
    pub async fn install(
        self,
        command_queue: CommandDispatcher,
    ) -> Result<(), CommandDispatcherError<InstallPixiEnvironmentError>> {
        // Split into source and binary records
        let (source_records, mut binary_records): (Vec<_>, Vec<_>) = self
            .records
            .into_iter()
            .partition_map(|record| match record {
                PixiRecord::Source(record) => Either::Left(record),
                PixiRecord::Binary(record) => Either::Right(record),
            });

        // Determine which packages are already installed.
        let installed_packages_fut = detect_installed_packages(&self.prefix);

        // Build all the source packages
        binary_records.reserve(source_records.len());
        let (tool_platform, tool_virtual_packages) = command_queue.tool_platform();
        for source_record in source_records {
            // Build the source package.
            let built_source = command_queue
                .source_build(SourceBuildSpec {
                    source: source_record.clone(),
                    channel_config: self.channel_config.clone(),
                    channels: self.channels.clone(),
                    host_platform: Some(PlatformAndVirtualPackages {
                        platform: tool_platform,
                        virtual_packages: Some(tool_virtual_packages.to_vec()),
                    }),
                    variants: self.variants.clone(),
                    enabled_protocols: self.enabled_protocols.clone(),
                })
                .await
                .map_err_with(|err| {
                    InstallPixiEnvironmentError::BuildError(source_record.source.to_string(), err)
                })?;

            // Determine the SHA256 hash of the built package.
            let sha = compute_package_sha256(&built_source.output_file).await?;

            // Update the metadata of the source package with information from the package
            // itself.
            let mut package_record = source_record.package_record.clone();
            package_record.sha256 = Some(sha);
            package_record.timestamp.get_or_insert_with(Utc::now);

            // Construct a repodata record which also includes information about where the
            // package is located.
            let repodata_record = RepoDataRecord {
                package_record,
                url: match Url::from_file_path(&built_source.output_file) {
                    Ok(url) => url,
                    Err(_) => panic!(
                        "failed to convert {} to URL",
                        built_source.output_file.display()
                    ),
                },
                channel: None,
                file_name: built_source
                    .output_file
                    .file_name()
                    .and_then(OsStr::to_str)
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            };

            // Add the repodata record of the source record to the binary records.
            binary_records.push(repodata_record);
        }

        // Wait for the installed packages here.
        let installed_packages = installed_packages_fut.await?;

        // Install the environment using the prefix installer
        let mut installer = Installer::new()
            .with_target_platform(self.target_platform)
            .with_download_client(command_queue.download_client().clone())
            .with_package_cache(command_queue.package_cache().clone())
            .with_reinstall_packages(self.force_reinstall)
            .with_installed_packages(installed_packages);

        if let Some(installed) = self.installed {
            installer = installer.with_installed_packages(installed);
        };

        let _result = installer
            .install(self.prefix.path(), binary_records)
            .await
            .map_err(InstallPixiEnvironmentError::Installer)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(())
    }
}

/// Detects the currently installed packages in the given prefix.
async fn detect_installed_packages(
    prefix: &Prefix,
) -> Result<Vec<PrefixRecord>, CommandDispatcherError<InstallPixiEnvironmentError>> {
    let path = prefix.path().to_path_buf();
    simple_spawn_blocking::tokio::run_blocking_task(move || {
        PrefixRecord::collect_from_prefix(&path).map_err(|e| {
            CommandDispatcherError::Failed(InstallPixiEnvironmentError::Installer(
                InstallerError::FailedToDetectInstalledPackages(e),
            ))
        })
    })
    .await
}

/// Computes the SHA256 hash of the package at the given path in a separate
/// thread.
async fn compute_package_sha256(
    package_path: &Path,
) -> Result<Sha256Hash, CommandDispatcherError<InstallPixiEnvironmentError>> {
    let path = package_path.to_path_buf();
    simple_spawn_blocking::tokio::run_blocking_task(move || {
        rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&path).map_err(|e| {
            CommandDispatcherError::Failed(InstallPixiEnvironmentError::CalculateSha256(path, e))
        })
    })
    .await
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstallPixiEnvironmentError {
    #[error(transparent)]
    Installer(InstallerError),

    #[error("failed to build a package for {0}")]
    BuildError(
        String,
        #[diagnostic_source]
        #[source]
        SourceBuildError,
    ),

    #[error("failed to calculate sha256 hash of {}", .0.display())]
    CalculateSha256(std::path::PathBuf, #[source] std::io::Error),
}
