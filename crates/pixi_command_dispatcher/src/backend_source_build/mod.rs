//! See [`BackendSourceBuildSpec`]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    path::PathBuf,
    str::FromStr,
};

use futures::{SinkExt, channel::mpsc::UnboundedSender};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use pixi_build_frontend::{Backend, json_rpc::CommunicationError};
use pixi_build_types::{
    ChannelConfiguration, PlatformAndVirtualPackages,
    procedures::{
        conda_build_v0::{CondaBuildParams, CondaBuiltPackage, CondaOutputIdentifier},
        conda_build_v1::{
            CondaBuildV1Dependency, CondaBuildV1DependencyRunExportSource,
            CondaBuildV1DependencySource, CondaBuildV1Output, CondaBuildV1Params,
            CondaBuildV1Prefix, CondaBuildV1PrefixPackage, CondaBuildV1Result,
            CondaBuildV1RunExports,
        },
    },
};
use pixi_record::PinnedSourceSpec;
use pixi_spec::{BinarySpec, PixiSpec, SpecConversionError};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, MatchSpec, PackageName, Platform, RepoDataRecord, Version,
};
use serde::Serialize;
use thiserror::Error;

use crate::{
    BuildEnvironment, CommandDispatcherError, PackageIdentifier,
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

    /// The package that we are building.
    pub package: PackageIdentifier,

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
    BuildV0(BackendSourceBuildV0Method),
    BuildV1(BackendSourceBuildV1Method),
}

#[derive(Debug, Serialize)]
pub struct BackendSourceBuildV0Method {
    /// Information about the platform to install build tools for and the
    /// platform to target.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The directory where to place the built package. This is used as a hint
    /// for the backend, it may still place the package elsewhere.
    pub output_directory: Option<PathBuf>,

    /// Whether to build the package in editable mode.
    pub editable: bool,
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
    pub variant: BTreeMap<String, String>,

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
            BackendSourceBuildMethod::BuildV0(params) => {
                Self::build_v0(
                    self.backend,
                    self.package,
                    self.source,
                    params,
                    self.work_directory,
                    self.channels,
                    self.channel_config,
                    log_sink,
                )
                .await
            }
            BackendSourceBuildMethod::BuildV1(params) => {
                Self::build_v1(
                    self.backend,
                    self.package,
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
    async fn build_v0(
        backend: Backend,
        record: PackageIdentifier,
        source: PinnedSourceSpec,
        params: BackendSourceBuildV0Method,
        work_directory: PathBuf,
        channels: Vec<ChannelUrl>,
        channel_config: ChannelConfig,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        // Use the backend to build the source package.
        let mut build_result = backend
            .conda_build_v0(
                CondaBuildParams {
                    build_platform_virtual_packages: Some(
                        params.build_environment.build_virtual_packages,
                    ),
                    channel_base_urls: Some(channels.into_iter().map(Into::into).collect()),
                    channel_configuration: ChannelConfiguration {
                        base_url: channel_config.channel_alias.clone(),
                    },
                    outputs: Some(BTreeSet::from_iter([CondaOutputIdentifier {
                        name: Some(record.name.as_normalized().to_string()),
                        version: Some(record.version.to_string()),
                        build: Some(record.build.clone()),
                        subdir: Some(record.subdir.clone()),
                    }])),
                    variant_configuration: params
                        .variants
                        .map(|variants| variants.into_iter().collect()),
                    work_directory: work_directory.clone(),
                    host_platform: Some(PlatformAndVirtualPackages {
                        platform: params.build_environment.host_platform,
                        virtual_packages: Some(
                            params.build_environment.host_virtual_packages.clone(),
                        ),
                    }),
                    editable: params.editable,
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
                    pkg.subdir,
                    pkg.name.as_normalized(),
                    pkg.version,
                    pkg.build,
                ))
            });
            tracing::warn!(
                "While building {} for {}, the build backend returned more packages than expected: {pkgs}. Only the package matching the source record will be used.",
                source,
                record.subdir,
            );
        }

        // Locate the package that matches the source record we requested to be build.
        let built_package = if let Some(idx) = build_result
            .packages
            .iter()
            .position(|pkg| v0_built_package_matches_request(&record, &pkg))
        {
            build_result.packages.swap_remove(idx)
        } else {
            return Err(CommandDispatcherError::Failed(
                BackendSourceBuildError::UnexpectedPackage(UnexpectedPackageError {
                    subdir: record.subdir.clone(),
                    name: record.name.as_normalized().to_string(),
                    version: record.version.to_string(),
                    build: record.build.clone(),
                    packages: build_result
                        .packages
                        .iter()
                        .map(|pkg| {
                            format!(
                                "{}/{}={}={}",
                                pkg.subdir,
                                pkg.name.as_normalized(),
                                pkg.version,
                                pkg.build
                            )
                        })
                        .collect(),
                }),
            ));
        };

        Ok(BackendBuiltSource {
            input_globs: built_package.input_globs,
            output_file: built_package.output_file,
        })
    }

    async fn build_v1(
        backend: Backend,
        record: PackageIdentifier,
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
                        name: record.name.clone(),
                        version: Some(record.version.clone()),
                        build: Some(record.build.clone()),
                        subdir: record
                            .subdir
                            .parse()
                            .expect("found a package record with an unparsable subdir"),
                        variant: params.variant,
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

        // Make sure that the built package matches the expected output.
        if v1_built_package_matches_requested(&built_package, &record) {
            return Err(CommandDispatcherError::Failed(
                BackendSourceBuildError::UnexpectedPackage(UnexpectedPackageError {
                    subdir: record.subdir.clone(),
                    name: record.name.as_normalized().to_string(),
                    version: record.version.to_string(),
                    build: record.build.clone(),
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
            input_globs: built_package.input_globs,
            output_file: built_package.output_file,
        })
    }
}

/// Returns true if the requested package matches the one that was built by a
/// backend.
fn v0_built_package_matches_request(record: &PackageIdentifier, pkg: &&CondaBuiltPackage) -> bool {
    pkg.name == record.name
        && Version::from_str(&pkg.version).ok().as_ref() == Some(&record.version)
        && pkg.build == record.build
        && pkg.subdir == record.subdir
}

/// Returns true if the requested package matches the one that was built by a
/// backend.
fn v1_built_package_matches_requested(
    built_package: &CondaBuildV1Result,
    record: &PackageIdentifier,
) -> bool {
    built_package.name != record.name.as_normalized()
        || built_package.version != record.version
        || built_package.build != record.build
        || built_package.subdir.as_str() != record.subdir
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

    #[error(transparent)]
    UnexpectedPackage(UnexpectedPackageError),
}

/// An error that can occur when the build backend did not return the expected
/// package.
#[derive(Debug, Error)]
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
