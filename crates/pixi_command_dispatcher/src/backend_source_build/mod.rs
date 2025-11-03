//! See [`BackendSourceBuildSpec`]

use std::{collections::BTreeSet, path::PathBuf};

use futures::{SinkExt, channel::mpsc::UnboundedSender};
use itertools::Either;
use miette::Diagnostic;
use pixi_build_frontend::{Backend, json_rpc::CommunicationError};
use pixi_build_types::procedures::conda_build_v1::{
    CondaBuildV1Dependency, CondaBuildV1DependencyRunExportSource, CondaBuildV1DependencySource,
    CondaBuildV1Output, CondaBuildV1Params, CondaBuildV1Prefix, CondaBuildV1PrefixPackage,
    CondaBuildV1RunExports,
};
use pixi_record::PinnedSourceSpec;
use pixi_spec::{BinarySpec, PixiSpec, SpecConversionError};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, MatchSpec, PackageName, Platform, RepoDataRecord,
};
use serde::Serialize;

use crate::{
    CommandDispatcherError,
    build::{Dependencies, DependencySource, PixiRunExports, WithSource},
};

/// The `BackendSourceBuildSpec` struct is used to define the specifications for
/// building a source package using a pre-instantiated backend. This task
/// performs the actual build of a source package.
///
/// We want to severely limit the number of packages that we build at the same
/// time; therefore, this is a separate task.
#[derive(Debug, Serialize)]
pub struct BackendSourceBuildSpec {
    /// The backend to use for the build.
    #[serde(skip)]
    pub backend: Backend,

    /// The name of the package to build
    pub package_name: PackageName,

    /// The platform to build the package for
    pub platform: Platform,

    /// The source location of the package that we are building.
    pub source: PinnedSourceSpec,

    /// The method to use for building the source package.
    pub method: BackendSourceBuildMethod,

    /// The channels to use for solving.
    pub channels: Vec<ChannelUrl>,

    /// The channel configuration to use to convert channel urls.
    pub channel_config: ChannelConfig,

    /// The working directory to use for the build.
    pub work_directory: PathBuf,
}

#[derive(Debug, Serialize)]
pub enum BackendSourceBuildMethod {
    BuildV1(BackendSourceBuildV1Method),
}

#[derive(Debug, Serialize)]
pub struct BackendSourceBuildV1Method {
    /// The build prefix that was prepared for the backend.
    pub build_prefix: BackendSourceBuildPrefix,

    /// The host prefix that was prepared for the backend.
    pub host_prefix: BackendSourceBuildPrefix,

    /// The run dependencies and constraints
    pub dependencies: Dependencies,

    /// The run exports
    pub run_exports: PixiRunExports,

    /// The variant to build
    /// TODO: This should move to the `SourceRecord` in the future. The variant
    /// is an essential part to identity a particular output of a source
    /// package.
    pub variant: crate::SelectedVariant,

    /// The directory where to place the built package. This is used as a hint
    /// for the backend, it may still place the package elsewhere.
    pub output_directory: Option<PathBuf>,

    /// Whether to build the package in editable mode.
    pub editable: bool,
}

#[derive(Debug, Serialize)]
pub struct BackendSourceBuildPrefix {
    /// The platform for which the packages were installed.
    pub platform: Platform,

    /// The location of the prefix on disk.
    #[serde(skip)]
    pub prefix: PathBuf,

    /// The records that are installed in the prefix.
    pub records: Vec<RepoDataRecord>,

    /// The dependencies that were used to solve the prefix.
    pub dependencies: Dependencies,
}

#[derive(Debug, Serialize)]
pub struct BackendBuiltSource {
    /// The location on disk where the built package is located.
    #[serde(skip)]
    pub output_file: PathBuf,

    /// The globs that were used as input to the build. Use these for
    /// re-verifying the build.
    pub input_globs: BTreeSet<String>,
}

impl BackendSourceBuildSpec {
    pub async fn build(
        self,
        log_sink: UnboundedSender<String>,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        match self.method {
            BackendSourceBuildMethod::BuildV1(params) => {
                Self::build_v1(
                    self.backend,
                    self.package_name,
                    self.platform,
                    params,
                    self.work_directory,
                    self.channels,
                    self.channel_config,
                    log_sink,
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_v1(
        backend: Backend,
        package_name: PackageName,
        platform: Platform,
        params: BackendSourceBuildV1Method,
        work_directory: PathBuf,
        channels: Vec<ChannelUrl>,
        channel_config: ChannelConfig,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        let built_package = backend
            .conda_build_v1(
                CondaBuildV1Params {
                    channels,
                    run_dependencies: Some(dependencies_to_protocol(
                        params.dependencies.dependencies.into_specs(),
                        &channel_config,
                    )),
                    run_constraints: Some(constraints_to_protocol(
                        params.dependencies.constraints.into_specs(),
                        &channel_config,
                    )),
                    run_exports: Some(CondaBuildV1RunExports {
                        weak: dependencies_to_protocol(
                            params
                                .run_exports
                                .weak
                                .into_specs()
                                .map(|(name, spec)| (name, WithSource::new(spec))),
                            &channel_config,
                        ),
                        strong: dependencies_to_protocol(
                            params
                                .run_exports
                                .strong
                                .into_specs()
                                .map(|(name, spec)| (name, WithSource::new(spec))),
                            &channel_config,
                        ),
                        noarch: dependencies_to_protocol(
                            params
                                .run_exports
                                .noarch
                                .into_specs()
                                .map(|(name, spec)| (name, WithSource::new(spec))),
                            &channel_config,
                        ),
                        weak_constrains: constraints_to_protocol(
                            params
                                .run_exports
                                .weak_constrains
                                .into_specs()
                                .map(|(name, spec)| (name, WithSource::new(spec))),
                            &channel_config,
                        ),
                        strong_constrains: constraints_to_protocol(
                            params
                                .run_exports
                                .strong_constrains
                                .into_specs()
                                .map(|(name, spec)| (name, WithSource::new(spec))),
                            &channel_config,
                        ),
                    }),
                    build_prefix: Some(CondaBuildV1Prefix {
                        prefix: params.build_prefix.prefix,
                        platform: params.build_prefix.platform,
                        dependencies: dependencies_to_protocol(
                            params.build_prefix.dependencies.dependencies.into_specs(),
                            &channel_config,
                        ),
                        constraints: constraints_to_protocol(
                            params.build_prefix.dependencies.constraints.into_specs(),
                            &channel_config,
                        ),
                        packages: params
                            .build_prefix
                            .records
                            .into_iter()
                            .map(|record| CondaBuildV1PrefixPackage {
                                repodata_record: record,
                            })
                            .collect(),
                    }),
                    host_prefix: Some(CondaBuildV1Prefix {
                        prefix: params.host_prefix.prefix,
                        platform: params.host_prefix.platform,
                        dependencies: dependencies_to_protocol(
                            params.host_prefix.dependencies.dependencies.into_specs(),
                            &channel_config,
                        ),
                        constraints: constraints_to_protocol(
                            params.host_prefix.dependencies.constraints.into_specs(),
                            &channel_config,
                        ),
                        packages: params
                            .host_prefix
                            .records
                            .into_iter()
                            .map(|record| CondaBuildV1PrefixPackage {
                                repodata_record: record,
                            })
                            .collect(),
                    }),
                    output: CondaBuildV1Output {
                        name: package_name,
                        version: None,
                        build: None,
                        subdir: platform,
                        variant: params.variant.into(),
                    },
                    work_directory: work_directory.clone(),
                    output_directory: params.output_directory,
                    editable: Some(params.editable),
                },
                move |line| {
                    let _err = futures::executor::block_on(log_sink.send(line));
                },
            )
            .await
            .map_err(BackendSourceBuildError::BuildError)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(BackendBuiltSource {
            input_globs: built_package.input_globs,
            output_file: built_package.output_file,
        })
    }
}

fn dependencies_to_protocol(
    dependencies: impl IntoIterator<Item = (PackageName, WithSource<PixiSpec>)>,
    channel_config: &ChannelConfig,
) -> Vec<CondaBuildV1Dependency> {
    dependencies
        .into_iter()
        .filter_map(|(name, spec)| {
            Some(CondaBuildV1Dependency {
                spec: convert_pixi_spec_to_match_spec(name, spec.value, channel_config).ok()?,
                source: spec.source.map(convert_source_to_protocol),
            })
        })
        .collect()
}

fn constraints_to_protocol(
    dependencies: impl IntoIterator<Item = (PackageName, WithSource<BinarySpec>)>,
    channel_config: &ChannelConfig,
) -> Vec<CondaBuildV1Dependency> {
    dependencies
        .into_iter()
        .filter_map(|(name, spec)| {
            Some(CondaBuildV1Dependency {
                spec: convert_binary_spec_to_match_spec(name, spec.value, channel_config).ok()?,
                source: spec.source.map(convert_source_to_protocol),
            })
        })
        .collect()
}

fn convert_source_to_protocol(dependency_source: DependencySource) -> CondaBuildV1DependencySource {
    match dependency_source {
        DependencySource::RunExport { name, env } => {
            CondaBuildV1DependencySource::RunExport(CondaBuildV1DependencyRunExportSource {
                from: env.to_string(),
                package_name: name,
            })
        }
    }
}

fn convert_pixi_spec_to_match_spec(
    package_name: PackageName,
    spec: PixiSpec,
    channel_config: &ChannelConfig,
) -> Result<MatchSpec, SpecConversionError> {
    let nameless_spec = match spec.into_source_or_binary() {
        Either::Left(source) => source.to_nameless_match_spec(),
        Either::Right(binary) => binary.try_into_nameless_match_spec(channel_config)?,
    };
    Ok(MatchSpec::from_nameless(nameless_spec, Some(package_name)))
}

fn convert_binary_spec_to_match_spec(
    package_name: PackageName,
    spec: BinarySpec,
    channel_config: &ChannelConfig,
) -> Result<MatchSpec, SpecConversionError> {
    let nameless_spec = spec.try_into_nameless_match_spec(channel_config)?;
    Ok(MatchSpec::from_nameless(nameless_spec, Some(package_name)))
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum BackendSourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildError(#[from] CommunicationError),
}
