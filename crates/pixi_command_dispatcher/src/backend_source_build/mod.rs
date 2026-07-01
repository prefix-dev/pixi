//! See [`BackendSourceBuildSpec`]

mod ext;

pub use ext::BackendSourceBuildExt;

use std::{collections::BTreeMap, fmt::Display, path::PathBuf, sync::Arc};

use crate::BackendHandle;
use futures::{SinkExt, channel::mpsc::UnboundedSender};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_frontend::json_rpc::CommunicationError;
use pixi_build_types::{
    ExtraGroupName, InputGlobSet, VariantValue,
    procedures::conda_build_v1::{
        CondaBuildV1Dependency, CondaBuildV1DependencyRunExportSource,
        CondaBuildV1DependencySource, CondaBuildV1Output, CondaBuildV1Params, CondaBuildV1Prefix,
        CondaBuildV1PrefixPackage, CondaBuildV1RunExports, CondaPackageFormat,
    },
};
use pixi_spec::{BinarySpec, PixiSpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, MatchSpec, PackageName, Platform, RepoDataRecord, VersionWithSource,
};
use serde::Serialize;
use thiserror::Error;

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
    pub backend: BackendHandle,

    /// The name of the package that we are building.
    pub name: PackageName,

    /// The version of the package.
    pub version: VersionWithSource,

    /// The build string of the package.
    pub build: String,

    /// The subdirectory (platform) of the package.
    pub subdir: String,

    /// The method to use for building the source package.
    pub method: BackendSourceBuildMethod,

    /// The channels to use for solving.
    pub channels: Vec<ChannelUrl>,

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

    /// The extra dependency groups, keyed by group name.
    pub extra_dependencies: BTreeMap<ExtraGroupName, DependencyMap<PackageName, PixiSpec>>,

    /// The run exports
    pub run_exports: PixiRunExports,

    /// The variant to build
    pub variant: BTreeMap<String, VariantValue>,

    /// The directory where to place the built package. This is used as a hint
    /// for the backend, it may still place the package elsewhere.
    pub output_directory: Option<PathBuf>,

    /// Whether to build the package in editable mode.
    pub editable: bool,

    /// Archive format and compression level. `None` lets the backend pick.
    pub package_format: Option<CondaPackageFormat>,
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

    /// The inputs the build read, as structured glob groups (patterns plus
    /// marker / hidden-file / root config). The backend's flat `input_globs`
    /// are folded into a group here so there's a single representation.
    pub input_glob_sets: Vec<InputGlobSet>,
}

impl BackendSourceBuildSpec {
    pub async fn build(
        self,
        channel_config: Arc<ChannelConfig>,
        log_sink: UnboundedSender<String>,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        match self.method {
            BackendSourceBuildMethod::BuildV1(params) => {
                Self::build_v1(
                    self.backend,
                    self.name,
                    self.version,
                    self.build,
                    self.subdir,
                    params,
                    self.work_directory,
                    self.channels,
                    channel_config,
                    log_sink,
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_v1(
        backend: BackendHandle,
        name: PackageName,
        version: VersionWithSource,
        build: String,
        subdir: String,
        params: BackendSourceBuildV1Method,
        work_directory: PathBuf,
        channels: Vec<ChannelUrl>,
        channel_config: Arc<ChannelConfig>,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        let built_package = backend
            .lock()
            .await
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
                    extra_dependencies: params
                        .extra_dependencies
                        .into_iter()
                        .map(|(group, deps)| {
                            (
                                group,
                                dependencies_to_protocol(
                                    deps.into_specs()
                                        .map(|(name, spec)| (name, WithSource::new(spec))),
                                    &channel_config,
                                ),
                            )
                        })
                        .collect(),
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
                        name: name.clone(),
                        version: Some(version.clone()),
                        build: Some(build.clone()),
                        subdir: subdir
                            .parse()
                            .expect("found a package record with an unparsable subdir"),
                        variant: params.variant,
                    },
                    work_directory: work_directory.clone(),
                    output_directory: params.output_directory,
                    editable: Some(params.editable),
                    package_format: params.package_format,
                },
                move |line| {
                    let _err = futures::executor::block_on(log_sink.send(line));
                },
            )
            .await
            .map_err(BackendSourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Make sure that the built package matches the expected output.
        if built_package.name != name.as_normalized()
            || built_package.version != version
            || built_package.build != build
            || built_package.subdir.as_str() != subdir
        {
            return Err(CommandDispatcherError::Failed(
                BackendSourceBuildError::UnexpectedPackage(UnexpectedPackageError {
                    subdir: subdir.clone(),
                    name: name.as_normalized().to_string(),
                    version: version.to_string(),
                    build: build.clone(),
                    packages: vec![format!(
                        "{}/{}={}={}",
                        built_package.subdir,
                        built_package.name,
                        built_package.version,
                        built_package.build
                    )],
                }),
            ));
        };

        Ok(BackendBuiltSource {
            input_glob_sets: crate::input_globs::fold_input_globs(
                built_package.input_globs,
                built_package.input_glob_sets,
            ),
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
    Ok(MatchSpec::from_nameless(nameless_spec, package_name.into()))
}

fn convert_binary_spec_to_match_spec(
    package_name: PackageName,
    spec: BinarySpec,
    channel_config: &ChannelConfig,
) -> Result<MatchSpec, SpecConversionError> {
    let nameless_spec = spec.try_into_nameless_match_spec(channel_config)?;
    Ok(MatchSpec::from_nameless(nameless_spec, package_name.into()))
}

#[derive(Debug, Clone, thiserror::Error, Diagnostic)]
pub enum BackendSourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildError(Arc<CommunicationError>),

    #[error(transparent)]
    UnexpectedPackage(UnexpectedPackageError),

    #[error(transparent)]
    GlobSet(Arc<pixi_glob::GlobSetError>),
}

impl From<CommunicationError> for BackendSourceBuildError {
    fn from(err: CommunicationError) -> Self {
        Self::BuildError(Arc::new(err))
    }
}

impl From<pixi_glob::GlobSetError> for BackendSourceBuildError {
    fn from(err: pixi_glob::GlobSetError) -> Self {
        Self::GlobSet(Arc::new(err))
    }
}

/// An error that can occur when the build backend did not return the expected
/// package.
#[derive(Debug, Clone, Error)]
pub struct UnexpectedPackageError {
    pub subdir: String,
    pub name: String,
    pub version: String,
    pub build: String,
    pub packages: Vec<String>,
}

impl Display for UnexpectedPackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.packages.len() {
            0 => write!(
                f,
                "The build backend did not return any packages for {}/{}={}={}.",
                self.subdir, self.name, self.version, self.build
            ),
            1 => write!(
                f,
                "The build backend did not return the expected package: {}/{}={}={}. Instead the build backend returned {}.",
                self.subdir, self.name, self.version, self.build, self.packages[0]
            ),
            _ => write!(
                f,
                "The build backend did not return the expected package: {}/{}={}={}. Instead the following packages were returned:\n- {}",
                self.subdir,
                self.name,
                self.version,
                self.build,
                self.packages.iter().format("\n- ")
            ),
        }
    }
}
