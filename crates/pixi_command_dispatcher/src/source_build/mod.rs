use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use miette::Diagnostic;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::Backend;
use pixi_build_types::procedures::conda_outputs::CondaOutputsParams;
use pixi_record::{PinnedSourceSpec, PixiRecord};
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, InvalidPackageNameError, Platform, prefix::Prefix,
};
use thiserror::Error;
use tracing::instrument;

use crate::{
    BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildMethod,
    BackendSourceBuildPrefix, BackendSourceBuildSpec, BackendSourceBuildV0Method,
    BackendSourceBuildV1Method, BuildEnvironment, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, InstallPixiEnvironmentError, InstallPixiEnvironmentSpec,
    InstantiateBackendError, InstantiateBackendSpec, PixiEnvironmentSpec,
    SolvePixiEnvironmentError, SourceCheckout, SourceCheckoutError,
    build::{
        Dependencies, DependenciesError, MoveError, SourceRecordOrCheckout, WorkDirKey, move_file,
    },
    package_identifier::PackageIdentifier,
};

/// Describes all parameters required to build a conda package from a pixi
/// source package.
///
/// This task prepares the build environment for a source build and then
/// delegates the actual build to the backend through the
/// [`BackendSourceBuildSpec`]. This allows preparation (installing host, build,
/// envs) to progress concurrently while the actual building of the package can
/// be done serially.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceBuildSpec {
    /// The source to build
    pub package: PackageIdentifier,

    /// The location of the source code to build.
    pub source: PinnedSourceSpec,

    /// The channel configuration to use when resolving metadata
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about host platform on which the package is build. Note that
    /// a package might be targeting noarch in which case the host platform
    /// should be used.
    ///
    /// If this field is omitted the build backend will use the current
    /// platform.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The directory where to place the built package.
    pub output_directory: Option<PathBuf>,

    /// The working directory to use for the build. If this is `None` a
    /// deterministic workspace local directory will be used.
    pub work_directory: Option<PathBuf>,

    /// Whether the build directory should be cleared before building.
    pub clean: bool,

    /// The protocols that are enabled for this source
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

pub struct BuiltSource {
    /// The source checkout that was built
    pub source: SourceCheckout,

    /// The location on disk where the built package is located.
    pub output_file: PathBuf,

    /// The globs that were used as input to the build. Use these for
    /// re-verifying the build.
    pub input_globs: BTreeSet<String>,
}

impl SourceBuildSpec {
    #[instrument(skip_all, fields(
        source = %self.source,
        subdir = %self.package.subdir,
        name = %self.package.name.as_normalized(),
        version = %self.package.version,
        build = %self.package.build))]
    pub(crate) async fn build(
        mut self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<BuiltSource, CommandDispatcherError<SourceBuildError>> {
        tracing::debug!("Building package for source spec: {}", self.source);

        // Check out the source code.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(self.source.clone())
            .await
            .map_err_with(SourceBuildError::SourceCheckout)?;

        // Discover information about the build backend from the source code.
        let discovered_backend = DiscoveredBackend::discover(
            &source_checkout.path,
            &self.channel_config,
            &self.enabled_protocols,
        )
        .map_err(SourceBuildError::Discovery)
        .map_err(CommandDispatcherError::Failed)?;

        // Instantiate the backend with the discovered information.
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend.backend_spec,
                init_params: discovered_backend.init_params,
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
            .map_err_with(SourceBuildError::Initialize)?;

        // Determine the working directory for the build.
        let work_directory = match std::mem::take(&mut self.work_directory) {
            Some(work_directory) => work_directory,
            None => command_dispatcher.cache_dirs().working_dirs().join(
                WorkDirKey {
                    source: SourceRecordOrCheckout::Record {
                        pinned: self.source.clone(),
                        package_name: self.package.name.clone(),
                    },
                    host_platform: self.build_environment.host_platform,
                    build_backend: backend.identifier().to_string(),
                }
                .key(),
            ),
        };

        // Clean the working directory if requested.
        if self.clean {
            if let Err(err) = fs_err::remove_dir_all(&work_directory) {
                return Err(CommandDispatcherError::Failed(
                    SourceBuildError::CleanWorkingDirectory(work_directory, err),
                ));
            }
        }

        // Build the package based on the support backend capabilities.
        let output_directory = self.output_directory.clone();
        let mut built_source = if backend.capabilities().provides_conda_build_v1() {
            let built_package = self
                .build_v1(command_dispatcher, backend, work_directory)
                .await?;

            BuiltSource {
                source: source_checkout,
                input_globs: built_package.input_globs,
                output_file: built_package.output_file,
            }
        } else {
            let built_package = self
                .build_v0(command_dispatcher, backend, work_directory)
                .await?;

            BuiltSource {
                source: source_checkout,
                input_globs: built_package.input_globs,
                output_file: built_package.output_file,
            }
        };

        // Make sure the package resides in the output directory that was requested.
        if let Some(output_directory) = output_directory {
            // Create the output directory if it does not exist.
            fs_err::create_dir_all(&output_directory).map_err(|err| {
                CommandDispatcherError::Failed(SourceBuildError::CreateOutputDirectory(err))
            })?;

            // At this point, the directory should exist, so we can canonicalize the path.
            let output_directory = fs_err::canonicalize(&output_directory)
                .map_err(CommandDispatcherError::Failed)
                .map_err_with(SourceBuildError::CreateOutputDirectory)?;

            // The output file should also exist.
            let output_file = match fs_err::canonicalize(&built_source.output_file) {
                Ok(output_file) => output_file,
                Err(_err) => {
                    return Err(CommandDispatcherError::Failed(
                        SourceBuildError::MissingOutputFile(built_source.output_file),
                    ));
                }
            };

            if output_file.parent() != Some(&output_directory) {
                // Take the file name of the file and move it to the output directory.
                let file_name = built_source
                    .output_file
                    .file_name()
                    .expect("the build backend did not return a file name");
                let destination = output_directory.join(file_name);
                if let Err(err) = move_file(&output_file, &destination) {
                    return Err(CommandDispatcherError::Failed(SourceBuildError::Move(
                        output_file,
                        output_directory,
                        err,
                    )));
                }
                built_source.output_file = destination;
            }
        }

        Ok(built_source)
    }

    async fn build_v0(
        self,
        command_dispatcher: CommandDispatcher,
        backend: Backend,
        work_directory: PathBuf,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<SourceBuildError>> {
        command_dispatcher
            .backend_source_build(BackendSourceBuildSpec {
                backend,
                package: self.package,
                source: self.source,
                work_directory,
                method: BackendSourceBuildMethod::BuildV0(BackendSourceBuildV0Method {
                    channel_config: self.channel_config,
                    channels: self.channels,
                    build_environment: self.build_environment,
                    variants: self.variants,
                    output_directory: self.output_directory,
                }),
            })
            .await
            .map_err_with(SourceBuildError::from)
    }

    async fn build_v1(
        self,
        command_dispatcher: CommandDispatcher,
        backend: Backend,
        work_directory: PathBuf,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<SourceBuildError>> {
        let source_anchor = SourceAnchor::from(SourceSpec::from(self.source.clone()));
        let host_platform = self.build_environment.host_platform;
        let build_platform = self.build_environment.build_platform;

        // Request the metadata from the backend.
        // TODO: Can we somehow cache this metadata?
        let outputs = backend
            .conda_outputs(CondaOutputsParams {
                host_platform,
                build_platform,
                variant_configuration: self.variants.clone(),
                work_directory: work_directory.clone(),
            })
            .await
            .map_err(BackendSourceBuildError::BuildError)
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?;

        // Find the output that we want to build.
        let output = outputs
            .outputs
            .into_iter()
            .find(|output| {
                output.metadata.name == self.package.name
                    && output.metadata.version == self.package.version
                    && output.metadata.build == self.package.build
                    && output.metadata.subdir.as_str() == self.package.subdir
            })
            .ok_or_else(|| {
                CommandDispatcherError::Failed(SourceBuildError::MissingOutput {
                    subdir: self.package.subdir.clone(),
                    name: self.package.name.as_normalized().to_string(),
                    version: self.package.version.to_string(),
                    build: self.package.build.clone(),
                })
            })?;

        // Determine final directories for everything.
        let directories = Directories::new(&work_directory, host_platform);

        // Solve the build environment.
        let build_dependencies = output
            .build_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone())))
            .transpose()
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?
            .unwrap_or_default();
        let build_records = self
            .solve_dependencies(
                format!("{} (build)", self.package.name.as_source()),
                &command_dispatcher,
                build_dependencies.clone(),
                self.build_environment.to_build_from_build(),
            )
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceBuildError::SolveBuildEnvironment)?;
        let build_run_exports =
            build_dependencies.extract_run_exports(&build_records, &output.ignore_run_exports);

        // Solve the host environment for the output.
        let host_dependencies = output
            .host_dependencies
            .as_ref()
            .map(|deps| Dependencies::new(deps, Some(source_anchor.clone())))
            .transpose()
            .map_err(SourceBuildError::from)
            .map_err(CommandDispatcherError::Failed)?
            .unwrap_or_default()
            // Extend with the run exports from the build environment.
            .extend_with_run_exports_from_build(&build_run_exports);
        let host_records = self
            .solve_dependencies(
                format!("{} (host)", self.package.name.as_source()),
                &command_dispatcher,
                host_dependencies.clone(),
                self.build_environment.clone(),
            )
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceBuildError::SolveBuildEnvironment)?;

        // Install the build environment
        let _build_prefix = command_dispatcher
            .install_pixi_environment(InstallPixiEnvironmentSpec {
                name: format!("{} (build)", self.package.name.as_source()),
                records: build_records,
                prefix: Prefix::create(&directories.build_prefix)
                    .map_err(SourceBuildError::CreateBuildEnvironmentDirectory)
                    .map_err(CommandDispatcherError::Failed)?,
                installed: None,
                build_environment: self.build_environment.to_build_from_build(),
                force_reinstall: Default::default(),
                channels: self.channels.clone(),
                channel_config: self.channel_config.clone(),
                variants: self.variants.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceBuildError::InstallBuildEnvironment)?;

        // Install the host environment.
        let _host_prefix = command_dispatcher
            .install_pixi_environment(InstallPixiEnvironmentSpec {
                name: format!("{} (host)", self.package.name.as_source()),
                records: host_records,
                prefix: Prefix::create(&directories.host_prefix)
                    .map_err(SourceBuildError::CreateBuildEnvironmentDirectory)
                    .map_err(CommandDispatcherError::Failed)?,
                installed: None,
                build_environment: self.build_environment.to_build_from_build(),
                force_reinstall: Default::default(),
                channels: self.channels.clone(),
                channel_config: self.channel_config.clone(),
                variants: self.variants.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
            .map_err_with(Box::new)
            .map_err_with(SourceBuildError::InstallBuildEnvironment)?;

        command_dispatcher
            .backend_source_build(BackendSourceBuildSpec {
                backend,
                package: self.package,
                source: self.source,
                work_directory,
                method: BackendSourceBuildMethod::BuildV1(BackendSourceBuildV1Method {
                    build_prefix: BackendSourceBuildPrefix {
                        platform: self.build_environment.build_platform,
                        prefix: directories.build_prefix,
                    },
                    host_prefix: BackendSourceBuildPrefix {
                        platform: self.build_environment.host_platform,
                        prefix: directories.host_prefix,
                    },
                    variant: output.metadata.variant,
                    output_directory: self.output_directory,
                }),
            })
            .await
            .map_err_with(SourceBuildError::from)
    }

    async fn solve_dependencies(
        &self,
        name: String,
        command_dispatcher: &CommandDispatcher,
        dependencies: Dependencies,
        build_environment: BuildEnvironment,
    ) -> Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>> {
        if dependencies.dependencies.is_empty() {
            return Ok(vec![]);
        }
        command_dispatcher
            .solve_pixi_environment(PixiEnvironmentSpec {
                name: Some(name),
                dependencies: dependencies.dependencies,
                constraints: dependencies.constraints,
                installed: vec![], // TODO: To lock build environments, fill this.
                build_environment,
                channels: self.channels.clone(),
                strategy: Default::default(),
                channel_priority: Default::default(),
                exclude_newer: None,
                channel_config: self.channel_config.clone(),
                variants: self.variants.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
    }
}

pub struct Directories {
    host_prefix: PathBuf,
    build_prefix: PathBuf,
}

impl Directories {
    pub fn new(work_directory: &Path, host_platform: Platform) -> Self {
        const BUILD_DIR: &str = "bld";
        const HOST_ENV_DIR: &str = "host";
        const PLACEHOLDER_TEMPLATE_STR: &str = "_placehold";

        let build_prefix = work_directory.join(BUILD_DIR);
        let host_prefix = if host_platform.is_windows() {
            work_directory.join(HOST_ENV_DIR)
        } else {
            // On non-Windows platforms, the name of the host environment has to be exactly
            // 255 characters long for prefix replacement in rattler build to work
            // correctly. This code constructs a directory name padded with a
            // template string so its exactly 255 characters long.
            //
            // TODO: This is really an implementation detail of how backends are generally
            // implemented, but this code should not really live in pixi.
            const PLACEHOLDER_LENGTH: usize = 255;
            let mut placeholder = String::new();
            while placeholder.len() < PLACEHOLDER_LENGTH {
                placeholder.push_str(PLACEHOLDER_TEMPLATE_STR);
            }
            let placeholder = placeholder
                [0..PLACEHOLDER_LENGTH - work_directory.join(HOST_ENV_DIR).as_os_str().len()]
                .to_string();

            work_directory.join(format!("{HOST_ENV_DIR}{}", placeholder))
        };
        Self {
            host_prefix,
            build_prefix,
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum SourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(#[from] pixi_build_discovery::DiscoveryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Initialize(#[from] InstantiateBackendError),

    #[error("failed to solve the build environment")]
    SolveBuildEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error("failed to solve the host environment")]
    SolveHostEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error("failed to create the build environment directory")]
    CreateBuildEnvironmentDirectory(#[source] std::io::Error),

    #[error("failed to create the host environment directory")]
    CreateHostEnvironmentDirectory(#[source] std::io::Error),

    #[error("failed to install the build environment")]
    InstallBuildEnvironment(#[source] Box<InstallPixiEnvironmentError>),

    #[error("failed to install the host environment")]
    InstallHostEnvironment(#[source] Box<InstallPixiEnvironmentError>),

    #[error(
        "The build backend does not provide the requested output: {subdir}/{name}={version}={build}."
    )]
    MissingOutput {
        subdir: String,
        name: String,
        version: String,
        build: String,
    },

    #[error(
        "The build backend returned a path for the build package ({0}), but the path does not exist."
    )]
    MissingOutputFile(PathBuf),

    #[error("backend returned a dependency on an invalid package name: {0}")]
    InvalidPackageName(String, #[source] InvalidPackageNameError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendBuildError(#[from] BackendSourceBuildError),

    #[error("failed to clean the working directory: {0}")]
    CleanWorkingDirectory(PathBuf, #[source] std::io::Error),

    #[error("moving the built package from {0} to the output directory {1} failed")]
    Move(PathBuf, PathBuf, #[source] MoveError),

    #[error("failed to create the output directory")]
    CreateOutputDirectory(#[source] std::io::Error),
}

impl From<DependenciesError> for SourceBuildError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(name, error) => {
                SourceBuildError::InvalidPackageName(name, error)
            }
        }
    }
}
