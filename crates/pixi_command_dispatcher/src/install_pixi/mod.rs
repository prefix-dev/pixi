mod reporter;

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ffi::OsStr,
    path::Path,
};

use chrono::Utc;
use futures::{FutureExt, StreamExt};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_glob::GlobModificationTime;
use pixi_record::{PixiRecord, SourceRecord};
use rattler::install::{
    Installer, InstallerError, Transaction,
    link_script::{LinkScriptError, PrePostLinkResult},
};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, PrefixRecord, RepoDataRecord, prefix::Prefix,
};
use rattler_digest::Sha256Hash;
use thiserror::Error;
use tracing::instrument;
use url::Url;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    SourceBuildError, SourceBuildSpec, SourceCheckout, SourceCheckoutError,
    build::{BuildCacheError, BuildInput, CachedBuild, CachedBuildSourceInfo},
    executor::ExecutorFutures,
    install_pixi::reporter::WrappingInstallReporter,
};

/// A list of globs that should be ignored when calculating any input hash.
/// These are typically used for build artifacts that should not be included in
/// the input hash.
const DEFAULT_BUILD_IGNORE_GLOBS: &[&str] = &["!.pixi/**"];

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPixiEnvironmentSpec {
    /// A descriptive name of the environment.
    pub name: String,

    /// The specification of the environment to install.
    #[serde(skip)]
    pub records: Vec<PixiRecord>,

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

    /// The protocols that are enabled for source packages
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

/// The result of installing a Pixi environment.
pub struct InstallPixiEnvironmentResult {
    /// The transaction that was applied
    pub transaction: Transaction<PrefixRecord, RepoDataRecord>,

    /// The result of running pre link scripts. `None` if no
    /// pre-processing was performed, possibly because link scripts were
    /// disabled.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// The result of running post link scripts. `None` if no
    /// post-processing was performed, possibly because link scripts were
    /// disabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,
}

impl InstallPixiEnvironmentSpec {
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

        // Determine which packages are already installed.
        let installed_packages_fut = match self.installed.take() {
            Some(installed) => std::future::ready(Ok(installed)).left_future(),
            None => detect_installed_packages(&self.prefix).right_future(),
        };

        // Build all the source packages concurrently.
        binary_records.reserve(source_records.len());
        let mut build_futures = ExecutorFutures::new(command_dispatcher.executor());
        for source_record in source_records {
            build_futures.push(async {
                self.build_from_source_with_cache(&command_dispatcher, &source_record)
                    .await
                    .map_err_with(move |build_err| {
                        InstallPixiEnvironmentError::BuildSourceError(source_record, build_err)
                    })
            });
        }
        while let Some(build_result) = build_futures.next().await {
            binary_records.push(build_result?);
        }
        drop(build_futures);

        // Wait for the installed packages here.
        let installed_packages = installed_packages_fut.await?;

        // Install the environment using the prefix installer
        let mut installer = Installer::new()
            .with_target_platform(self.build_environment.host_platform)
            .with_download_client(command_dispatcher.download_client().clone())
            .with_package_cache(command_dispatcher.package_cache().clone())
            .with_reinstall_packages(self.force_reinstall)
            .with_execute_link_scripts(command_dispatcher.allow_execute_link_scripts())
            .with_installed_packages(installed_packages);

        if let Some(installed) = self.installed {
            installer = installer.with_installed_packages(installed);
        };

        if let Some(reporter) = install_reporter {
            installer = installer.with_reporter(WrappingInstallReporter(reporter));
        }

        let result = installer
            .install(self.prefix.path(), binary_records)
            .await
            .map_err(InstallPixiEnvironmentError::Installer)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(InstallPixiEnvironmentResult {
            transaction: result.transaction,
            post_link_script_result: result.post_link_script_result,
            pre_link_script_result: result.pre_link_script_result,
        })
    }

    /// Builds a package from source or returns a cached build if it exists.
    #[instrument(skip_all, fields(
        source = %source_record.source,
        subdir = %source_record.package_record.subdir,
        name = %source_record.package_record.name.as_normalized(),
        version = %source_record.package_record.version,
        build = %source_record.package_record.build))
    ]
    async fn build_from_source_with_cache(
        &self,
        command_dispatcher: &CommandDispatcher,
        source_record: &SourceRecord,
    ) -> Result<RepoDataRecord, CommandDispatcherError<BuildSourceError>> {
        // Check the build cache for an existing build.
        let (tool_platform, tool_virtual_packages) = command_dispatcher.tool_platform();
        let build_input = BuildInput {
            channel_urls: self.channels.clone(),
            name: source_record.package_record.name.as_source().to_string(),
            version: source_record.package_record.version.to_string(),
            build: source_record.package_record.build.to_string(),
            subdir: source_record.package_record.subdir.clone(),
            host_platform: tool_platform,
            host_virtual_packages: tool_virtual_packages.to_vec(),
            build_virtual_packages: tool_virtual_packages.to_vec(),
        };
        let (cached_build, build_cache_entry) = command_dispatcher
            .build_cache()
            .entry(&source_record.source, &build_input)
            .await
            .map_err(BuildSourceError::BuildCacheError)
            .map_err(CommandDispatcherError::Failed)?;

        // If we have a cached entry, verify that it is still valid.
        if let Some(cached_build) = cached_build {
            if !self
                .force_reinstall
                .contains(&source_record.package_record.name)
                && self
                    .verify_cached_build(command_dispatcher, &cached_build, source_record)
                    .await?
            {
                return Ok(cached_build.record);
            }
        }

        // Otherwise, build the package from source
        let (repodata_record, input_globs, source_checkout) = self
            .build_from_source(
                command_dispatcher,
                source_record,
                build_cache_entry.cache_dir(),
            )
            .await?;

        // Store the built package in the cache. This will modify the location of the
        // package, the returned updated repodata record will reflect that.
        let repodata_record = build_cache_entry
            .insert(CachedBuild {
                source: if !source_checkout.pinned.is_immutable() {
                    Some(CachedBuildSourceInfo { globs: input_globs })
                } else {
                    None
                },
                record: repodata_record.clone(),
            })
            .await
            .map_err(BuildSourceError::BuildCacheError)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(repodata_record)
    }

    /// Given a particular source record, build the package from source.
    ///
    /// This function does not perform any caching, use
    /// `build_from_source_with_cache` if you want to use a cache.
    async fn build_from_source(
        &self,
        command_dispatcher: &CommandDispatcher,
        source_record: &SourceRecord,
        output_directory: &Path,
    ) -> Result<
        (RepoDataRecord, BTreeSet<String>, SourceCheckout),
        CommandDispatcherError<BuildSourceError>,
    > {
        // Build the source package.
        let built_source = command_dispatcher
            .source_build(SourceBuildSpec {
                source: source_record.clone(),
                channel_config: self.channel_config.clone(),
                channels: self.channels.clone(),
                build_environment: self.build_environment.clone(),
                variants: self.variants.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
                output_directory: Some(output_directory.to_path_buf()),
            })
            .await
            .map_err_with(BuildSourceError::BuildError)?;

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

        Ok((
            repodata_record,
            built_source.input_globs,
            built_source.source,
        ))
    }

    /// Given a cached build, verify that it is still valid for the given source
    /// record.
    async fn verify_cached_build(
        &self,
        command_dispatcher: &CommandDispatcher,
        cached_build: &CachedBuild,
        source_record: &SourceRecord,
    ) -> Result<bool, CommandDispatcherError<BuildSourceError>> {
        // Immutable source records are always considered valid.
        if source_record.source.is_immutable() {
            return Ok(true);
        }

        // If there are no source globs, we always consider the cached package
        // up-to-date.
        let Some(source_info) = &cached_build.source else {
            return Ok(true);
        };
        if source_info.globs.is_empty() {
            return Ok(true);
        }

        // Checkout the source for the package.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(source_record.source.clone())
            .await
            .map_err_with(BuildSourceError::SourceCheckoutError)?;

        // Compute the modification time of the files that match the source input globs.
        let glob_time = match GlobModificationTime::from_patterns(
            &source_checkout.path,
            source_info
                .globs
                .iter()
                .map(String::as_str)
                .chain(DEFAULT_BUILD_IGNORE_GLOBS.iter().copied()),
        ) {
            Ok(glob_time) => glob_time,
            Err(e) => {
                tracing::warn!(
                    "failed to determine modification time of input files: {}. Assuming the package is out-of-date.",
                    e
                );
                return Ok(false);
            }
        };

        match glob_time {
            GlobModificationTime::MatchesFound {
                modified_at,
                designated_file,
            } => {
                if cached_build
                    .record
                    .package_record
                    .timestamp
                    .map(|t| t >= chrono::DateTime::<Utc>::from(modified_at))
                    .unwrap_or(false)
                {
                    tracing::debug!("found an up-to-date cached build.");
                    return Ok(true);
                } else {
                    tracing::debug!(
                        "found an stale cached build, {} is newer than {}",
                        designated_file.display(),
                        cached_build
                            .record
                            .package_record
                            .timestamp
                            .unwrap_or_default()
                    );
                }
            }
            GlobModificationTime::NoMatches => {
                // No matches, so we should rebuild.
                tracing::debug!("found a stale cached build, no files match the source glob");
            }
        }

        // The package record cannot be valid.
        Ok(false)
    }
}

/// Detects the currently installed packages in the given prefix.
async fn detect_installed_packages(
    prefix: &Prefix,
) -> Result<Vec<PrefixRecord>, CommandDispatcherError<InstallPixiEnvironmentError>> {
    let prefix = prefix.clone();
    simple_spawn_blocking::tokio::run_blocking_task(move || {
        PrefixRecord::collect_from_prefix(prefix.path()).map_err(|e| {
            CommandDispatcherError::Failed(InstallPixiEnvironmentError::ReadInstalledPackages(
                prefix, e,
            ))
        })
    })
    .await
}

/// Computes the SHA256 hash of the package at the given path in a separate
/// thread.
async fn compute_package_sha256(
    package_path: &Path,
) -> Result<Sha256Hash, CommandDispatcherError<BuildSourceError>> {
    let path = package_path.to_path_buf();
    simple_spawn_blocking::tokio::run_blocking_task(move || {
        rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&path)
            .map_err(|e| CommandDispatcherError::Failed(BuildSourceError::CalculateSha256(path, e)))
    })
    .await
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
        .0.source)]
    BuildSourceError(
        SourceRecord,
        #[diagnostic_source]
        #[source]
        BuildSourceError,
    ),
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildSourceError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildError(#[from] SourceBuildError),

    #[error("failed to calculate sha256 hash of {}", .0.display())]
    CalculateSha256(std::path::PathBuf, #[source] std::io::Error),

    #[error(transparent)]
    BuildCacheError(BuildCacheError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckoutError(SourceCheckoutError),
}
