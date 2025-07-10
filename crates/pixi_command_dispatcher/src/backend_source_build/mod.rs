//! See [`BackendSourceBuildSpec`]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    str::FromStr,
};

use futures::{SinkExt, channel::mpsc::UnboundedSender};
use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_frontend::{Backend, json_rpc::CommunicationError};
use pixi_build_types::{
    ChannelConfiguration, PlatformAndVirtualPackages,
    procedures::{
        conda_build::{CondaBuildParams, CondaOutputIdentifier},
        conda_build_v2::{CondaBuildV2Output, CondaBuildV2Params, CondaBuildV2Prefix},
    },
};
use pixi_record::SourceRecord;
use rattler_conda_types::{ChannelConfig, ChannelUrl, Platform, Version};

use crate::{BuildEnvironment, CommandDispatcher, CommandDispatcherError, build::WorkDirKey};

/// The `BackendSourceBuildSpec` struct is used to define the specifications for
/// building a source package using a pre-instantiated backend. This task
/// performs the actual build of a source package.
///
/// We want to severely limit the number of packages that we build at the same
/// time; therefore, this is a separate task.
pub struct BackendSourceBuildSpec {
    /// The backend to use for the build.
    pub backend: Backend,

    /// The package that we are building.
    pub record: SourceRecord,

    /// The method to use for building the source package.
    pub method: BackendSourceBuildMethod,
}

pub enum BackendSourceBuildMethod {
    BuildV1(BackendSourceBuildV1Method),
    BuildV2(BackendSourceBuildV2Method),
}

pub struct BackendSourceBuildV1Method {
    /// The channel configuration to use when resolving metadata
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    pub channels: Vec<ChannelUrl>,

    /// Information about the platform to install build tools for and the
    /// platform to target.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The directory where to place the built package. This is used as a hint
    /// for the backend, it may still place the package elsewhere.
    pub output_directory: Option<PathBuf>,
}

pub struct BackendSourceBuildV2Method {
    /// The build prefix that was prepared for the backend.
    pub build_prefix: BackendSourceBuildPrefix,

    /// The host prefix that was prepared for the backend.
    pub host_prefix: BackendSourceBuildPrefix,

    /// The variant to build
    /// TODO: This should move to the `SourceRecord` in the future.
    pub variant: BTreeMap<String, String>,

    /// The directory where to place the built package. This is used as a hint
    /// for the backend, it may still place the package elsewhere.
    pub output_directory: Option<PathBuf>,
}

pub struct BackendSourceBuildPrefix {
    /// The platform for which the packages were installed.
    pub platform: Platform,

    /// The location of the prefix on disk.
    pub prefix: PathBuf,
}

pub struct BackendBuiltSource {
    /// The location on disk where the built package is located.
    pub output_file: PathBuf,

    /// The globs that were used as input to the build. Use these for
    /// re-verifying the build.
    pub input_globs: BTreeSet<String>,
}

impl BackendSourceBuildSpec {
    pub async fn build(
        self,
        command_dispatcher: CommandDispatcher,
        log_sink: UnboundedSender<String>,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        match self.method {
            BackendSourceBuildMethod::BuildV1(params) => {
                Self::build_v1(
                    self.backend,
                    self.record,
                    params,
                    log_sink,
                    command_dispatcher,
                )
                .await
            }
            BackendSourceBuildMethod::BuildV2(params) => {
                Self::build_v2(
                    self.backend,
                    self.record,
                    params,
                    log_sink,
                    command_dispatcher,
                )
                .await
            }
        }
    }

    async fn build_v1(
        backend: Backend,
        record: SourceRecord,
        params: BackendSourceBuildV1Method,
        mut log_sink: UnboundedSender<String>,
        command_dispatcher: CommandDispatcher,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        // Use the backend to build the source package.
        let build_result = backend
            .conda_build(
                CondaBuildParams {
                    build_platform_virtual_packages: Some(
                        params.build_environment.build_virtual_packages,
                    ),
                    channel_base_urls: Some(params.channels.into_iter().map(Into::into).collect()),
                    channel_configuration: ChannelConfiguration {
                        base_url: params.channel_config.channel_alias.clone(),
                    },
                    outputs: Some(BTreeSet::from_iter([CondaOutputIdentifier {
                        name: Some(record.package_record.name.as_normalized().to_string()),
                        version: Some(record.package_record.version.to_string()),
                        build: Some(record.package_record.build.clone()),
                        subdir: Some(record.package_record.subdir.clone()),
                    }])),
                    variant_configuration: params
                        .variants
                        .map(|variants| variants.into_iter().collect()),
                    work_directory: command_dispatcher.cache_dirs().working_dirs().join(
                        WorkDirKey {
                            source: Box::new(record.clone()).into(),
                            host_platform: params.build_environment.host_platform,
                            build_backend: backend.identifier().to_string(),
                        }
                        .key(),
                    ),
                    host_platform: Some(PlatformAndVirtualPackages {
                        platform: params.build_environment.host_platform,
                        virtual_packages: Some(
                            params.build_environment.host_virtual_packages.clone(),
                        ),
                    }),
                    editable: record.source.is_immutable(),
                },
                move |line| {
                    let _err = futures::executor::block_on(log_sink.send(line));
                },
            )
            .await
            .map_err(BackendSourceBuildError::BuildError)
            .map_err(CommandDispatcherError::Failed)?;

        // If the backend returned more packages than expected output a warning.
        if build_result.packages.len() > 1 {
            let pkgs = build_result.packages.iter().format_with(", ", |pkg, f| {
                f(&format_args!(
                    "{}/{}={}={}",
                    pkg.subdir, pkg.name, pkg.version, pkg.build,
                ))
            });
            tracing::warn!(
                "While building {} for {}, the build backend returned more packages than expected: {pkgs}. Only the package matching the source record will be used.",
                record.source,
                record.package_record.subdir,
            );
        }

        // Locate the package that matches the source record we requested to be build.
        let Some(built_package) = build_result.packages.into_iter().find(|pkg| {
            pkg.name == record.package_record.name.as_normalized()
                && Version::from_str(&pkg.version).ok().as_ref()
                    == Some(&record.package_record.version)
                && pkg.build == record.package_record.build
                && pkg.subdir == record.package_record.subdir
        }) else {
            return Err(CommandDispatcherError::Failed(
                BackendSourceBuildError::UnexpectedPackage {
                    subdir: record.package_record.subdir.clone(),
                    name: record.package_record.name.as_normalized().to_string(),
                    version: record.package_record.version.to_string(),
                    build: record.package_record.build.clone(),
                },
            ));
        };

        Ok(BackendBuiltSource {
            input_globs: built_package.input_globs,
            output_file: built_package.output_file,
        })
    }

    async fn build_v2(
        backend: Backend,
        record: SourceRecord,
        params: BackendSourceBuildV2Method,
        mut log_sink: UnboundedSender<String>,
        command_dispatcher: CommandDispatcher,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        let work_directory = command_dispatcher.cache_dirs().working_dirs().join(
            WorkDirKey {
                source: Box::new(record.clone()).into(),
                host_platform: params.host_prefix.platform,
                build_backend: backend.identifier().to_string(),
            }
            .key(),
        );

        let built_package = backend
            .conda_build_v2(
                CondaBuildV2Params {
                    build_prefix: Some(CondaBuildV2Prefix {
                        prefix: params.build_prefix.prefix,
                        platform: params.build_prefix.platform,
                    }),
                    host_prefix: Some(CondaBuildV2Prefix {
                        prefix: params.host_prefix.prefix,
                        platform: params.host_prefix.platform,
                    }),
                    output: CondaBuildV2Output {
                        name: record.package_record.name.clone(),
                        version: Some(record.package_record.version.clone()),
                        build: Some(record.package_record.build.clone()),
                        subdir: record
                            .package_record
                            .subdir
                            .parse()
                            .expect("found a package record with an unparsable subdir"),
                        variant: params.variant,
                    },
                    work_directory,
                    output_directory: params.output_directory,
                    editable: Some(!record.source.is_immutable()),
                },
                move |line| {
                    let _err = futures::executor::block_on(log_sink.send(line));
                },
            )
            .await
            .map_err(BackendSourceBuildError::BuildError)
            .map_err(CommandDispatcherError::Failed)?;

        // Make sure that the built package matches the expected output.
        if built_package.name != record.package_record.name.as_normalized()
            || built_package.version != record.package_record.version
            || built_package.build != record.package_record.build
            || built_package.subdir.as_str() != record.package_record.subdir
        {
            return Err(CommandDispatcherError::Failed(
                BackendSourceBuildError::UnexpectedPackage {
                    subdir: record.package_record.subdir.clone(),
                    name: record.package_record.name.as_normalized().to_string(),
                    version: record.package_record.version.to_string(),
                    build: record.package_record.build.clone(),
                },
            ));
        };

        Ok(BackendBuiltSource {
            input_globs: built_package.input_globs,
            output_file: built_package.output_file,
        })
    }
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum BackendSourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildError(#[from] CommunicationError),

    #[error(
        "The build backend did not return the expected package: {subdir}/{name}={version}={build}."
    )]
    UnexpectedPackage {
        subdir: String,
        name: String,
        version: String,
        build: String,
    },
}
