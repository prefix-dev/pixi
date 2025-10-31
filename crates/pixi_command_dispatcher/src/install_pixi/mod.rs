mod reporter;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsStr,
    path::PathBuf,
};

use futures::StreamExt;
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_record::{PixiRecord, SourceRecord};
use rattler::install::{
    InstallationResultRecord, Installer, InstallerError, Transaction,
    link_script::{LinkScriptError, PrePostLinkResult},
};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, PackageName, PrefixRecord, RepoDataRecord, prefix::Prefix,
};
use thiserror::Error;

use crate::{
    BuildEnvironment, BuildProfile, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, SourceBuildError, SourceBuildSpec, executor::ExecutorFutures,
    install_pixi::reporter::WrappingInstallReporter,
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPixiEnvironmentSpec {
    /// A descriptive name of the environment.
    pub name: String,

    /// The specification of the environment to install.
    #[serde(skip)]
    pub records: Vec<PixiRecord>,

    /// The packages to ignore, meaning dont remove if not present in records
    /// do not update when also present in PixiRecord
    pub ignore_packages: Option<HashSet<PackageName>>,

    /// The location to create the prefix at.
    #[serde(skip)]
    pub prefix: Prefix,

    /// If already known, the installed packages
    #[serde(skip)]
    pub installed: Option<Vec<PrefixRecord>>,

    /// Describes the platform and how packages should be built for it.
    pub build_environment: BuildEnvironment,

    /// Packages to force reinstalling.
    #[serde(skip_serializing_if = "HashSet::is_empty")]
    pub force_reinstall: HashSet<rattler_conda_types::PackageName>,

    /// The channels to use when building source packages.
    pub channels: Vec<ChannelUrl>,

    /// The channel configuration to use for this environment.
    pub channel_config: ChannelConfig,

    /// Build variants to use during the solve
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// Build variant file contents to use during the solve
    pub variant_files: Option<Vec<PathBuf>>,

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

/// The result of installing a Pixi environment.
pub struct InstallPixiEnvironmentResult {
    /// The transaction that was applied
    pub transaction: Transaction<InstallationResultRecord, RepoDataRecord>,

    /// The result of running pre link scripts. `None` if no
    /// pre-processing was performed, possibly because link scripts were
    /// disabled.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// The result of running post link scripts. `None` if no
    /// post-processing was performed, possibly because link scripts were
    /// disabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// If source records where specified as part of the input they will be
    /// built. This map contains the resulting repodata record for a build
    /// source record.
    pub resolved_source_records: HashMap<PackageName, RepoDataRecord>,
}

impl InstallPixiEnvironmentSpec {
    pub fn new(records: Vec<PixiRecord>, prefix: Prefix) -> Self {
        InstallPixiEnvironmentSpec {
            name: prefix
                .file_name()
                .map(OsStr::to_string_lossy)
                .map(Cow::into_owned)
                .unwrap_or_default(),
            records,
            prefix,
            installed: None,
            ignore_packages: None,
            build_environment: BuildEnvironment::default(),
            force_reinstall: HashSet::new(),
            channels: Vec::new(),
            channel_config: ChannelConfig::default_with_root_dir(PathBuf::from(".")),
            variants: None,
            variant_files: None,
            enabled_protocols: EnabledProtocols::default(),
        }
    }

    pub async fn install(
        mut self,
        command_dispatcher: CommandDispatcher,
        install_reporter: Option<Box<dyn rattler::install::Reporter>>,
    ) -> Result<InstallPixiEnvironmentResult, CommandDispatcherError<InstallPixiEnvironmentError>>
    {
        // Split into source and binary records
        let (source_records, mut binary_records): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.records)
                .into_iter()
                .partition_map(|record| match record {
                    PixiRecord::Source(record) => Either::Left(record),
                    PixiRecord::Binary(record) => Either::Right(record),
                });

        // Build all the source packages concurrently.
        binary_records.reserve(source_records.len());
        let mut build_futures = ExecutorFutures::new(command_dispatcher.executor());
        for source_record in source_records {
            // Do not build if package is explicitly ignored
            if self
                .ignore_packages
                .as_ref()
                .is_some_and(|ignore| ignore.contains(&source_record.package_record.name))
            {
                continue;
            }
            build_futures.push(async {
                self.build_from_source(&command_dispatcher, &source_record)
                    .await
                    .map_err_with(move |build_err| {
                        InstallPixiEnvironmentError::BuildSourceError(
                            Box::new(source_record),
                            build_err,
                        )
                    })
            });
        }

        let mut resolved_source_records = HashMap::new();
        while let Some(build_result) = build_futures.next().await {
            let build_result = build_result?;
            resolved_source_records.insert(
                build_result.package_record.name.clone(),
                build_result.clone(),
            );
            binary_records.push(build_result);
        }
        drop(build_futures);

        // Install the environment using the prefix installer
        let mut installer = Installer::new()
            .with_target_platform(self.build_environment.host_platform)
            .with_download_client(command_dispatcher.download_client().clone())
            .with_package_cache(command_dispatcher.package_cache().clone())
            .with_reinstall_packages(self.force_reinstall)
            .with_ignored_packages(self.ignore_packages.unwrap_or_default())
            .with_execute_link_scripts(command_dispatcher.allow_execute_link_scripts());

        if let Some(installed) = self.installed {
            installer = installer.with_installed_packages(installed);
        };

        if let Some(reporter) = install_reporter {
            installer = installer.with_reporter(WrappingInstallReporter(reporter));
        }

        let result = installer
            .install(self.prefix.path(), binary_records)
            .await
            .map_err(|err| match err {
                InstallerError::FailedToDetectInstalledPackages(err) => {
                    InstallPixiEnvironmentError::ReadInstalledPackages(self.prefix, err)
                }
                err => InstallPixiEnvironmentError::Installer(err),
            })
            .map_err(CommandDispatcherError::Failed)?;

        Ok(InstallPixiEnvironmentResult {
            transaction: result.transaction,
            post_link_script_result: result.post_link_script_result,
            pre_link_script_result: result.pre_link_script_result,
            resolved_source_records,
        })
    }

    /// Given a particular source record, build the package from source.
    async fn build_from_source(
        &self,
        command_dispatcher: &CommandDispatcher,
        source_record: &SourceRecord,
    ) -> Result<RepoDataRecord, CommandDispatcherError<SourceBuildError>> {
        // Build the source package.
        // Verify if we need to force the build even if the cache is up to date.
        let force = self
            .force_reinstall
            .contains(&source_record.package_record.name);
        let built_source = command_dispatcher
            .source_build(SourceBuildSpec {
                manifest_source: source_record.manifest_source.clone(),
                package: source_record.into(),
                channel_config: self.channel_config.clone(),
                channels: self.channels.clone(),
                build_environment: self.build_environment.clone(),
                variants: self.variants.clone(),
                variant_files: self.variant_files.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
                output_directory: None,
                work_directory: None,
                clean: false,
                // Should we force the build even if the cache is up to date?
                force,
                // When we install a pixi environment we always build in development mode.
                build_profile: BuildProfile::Development,
                build_source: source_record.build_source.clone(),
            })
            .await?;

        Ok(built_source.record)
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstallPixiEnvironmentError {
    #[error("failed to collect prefix records from '{}'", .0.path().display())]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ReadInstalledPackages(Prefix, #[source] std::io::Error),

    #[error(transparent)]
    Installer(InstallerError),

    #[error("failed to build '{}' from '{}'",
        .0.package_record.name.as_source(),
        .0.manifest_source)]
    BuildSourceError(
        Box<SourceRecord>,
        #[diagnostic_source]
        #[source]
        SourceBuildError,
    ),

    #[error(
        "failed to convert install transaction to prefix records from '{}'",
        .0.path().display()
    )]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ConvertTransactionToPrefixRecord(Prefix, #[source] std::io::Error),
}
