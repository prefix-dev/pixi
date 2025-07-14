use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use miette::Diagnostic;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::Backend;
use pixi_build_types::procedures::conda_outputs::CondaOutputsParams;
use pixi_record::{PixiRecord, SourceRecord};
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, InvalidPackageNameError, Platform, prefix::Prefix,
};
use thiserror::Error;
use tracing::instrument;

use crate::{
    BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildMethod,
    BackendSourceBuildPrefix, BackendSourceBuildSpec, BackendSourceBuildV1Method,
    BackendSourceBuildV2Method, BuildEnvironment, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, InstallPixiEnvironmentError, InstallPixiEnvironmentSpec,
    InstantiateBackendError, InstantiateBackendSpec, PixiEnvironmentSpec,
    SolvePixiEnvironmentError, SourceCheckout, SourceCheckoutError,
    build::{Dependencies, DependenciesError, WorkDirKey},
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
    /// The source specification
    pub source: SourceRecord,

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

    /// The directory where to place the built package. This is used as a hint
    /// for the backend, it may still place the package elsewhere.
    pub output_directory: Option<PathBuf>,

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
        source = %self.source.source,
        subdir = %self.source.package_record.subdir,
        name = %self.source.package_record.name.as_normalized(),
        version = %self.source.package_record.version,
        build = %self.source.package_record.build))]
    pub(crate) async fn build(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<BuiltSource, CommandDispatcherError<SourceBuildError>> {
        tracing::debug!("Building package for source spec: {}", self.source.source);

        // Check out the source code.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(self.source.source.clone())
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

        if backend.capabilities().provides_conda_build_v1() {
            let built_package = self.build_v2(command_dispatcher, backend).await?;

            Ok(BuiltSource {
                source: source_checkout,
                input_globs: built_package.input_globs,
                output_file: built_package.output_file,
            })
        } else {
            let built_package = self.build_v1(command_dispatcher, backend).await?;

            Ok(BuiltSource {
                source: source_checkout,
                input_globs: built_package.input_globs,
                output_file: built_package.output_file,
            })
        }
    }

    async fn build_v1(
        self,
        command_dispatcher: CommandDispatcher,
        backend: Backend,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<SourceBuildError>> {
        command_dispatcher
            .backend_source_build(BackendSourceBuildSpec {
                backend,
                record: self.source,
                method: BackendSourceBuildMethod::BuildV1(BackendSourceBuildV1Method {
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

    async fn build_v2(
        self,
        command_dispatcher: CommandDispatcher,
        backend: Backend,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<SourceBuildError>> {
        let source_anchor = SourceAnchor::from(SourceSpec::from(self.source.source.clone()));
        let host_platform = self.build_environment.host_platform;
        let build_platform = self.build_environment.build_platform;

        // Determine the working directory for the build.
        let work_directory = command_dispatcher.cache_dirs().working_dirs().join(
            WorkDirKey {
                source: Box::new(self.source.clone()).into(),
                host_platform,
                build_backend: backend.identifier().to_string(),
            }
            .key(),
        );

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
                output.metadata.name == self.source.package_record.name
                    && output.metadata.version == self.source.package_record.version
                    && output.metadata.build == self.source.package_record.build
                    && output.metadata.subdir.as_str() == self.source.package_record.subdir
            })
            .ok_or_else(|| {
                CommandDispatcherError::Failed(SourceBuildError::MissingOutput {
                    subdir: self.source.package_record.subdir.clone(),
                    name: self.source.package_record.name.as_normalized().to_string(),
                    version: self.source.package_record.version.to_string(),
                    build: self.source.package_record.build.clone(),
                })
            })?;

        // Determine final directories for everything.
        // TODO: There is some magic in how we decide the name for the host prefix. This
        // magic should ideally be managed by the backend.
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
                format!("{} (build)", self.source.package_record.name.as_source()),
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
                format!("{} (host)", self.source.package_record.name.as_source()),
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
                name: format!("{} (build)", self.source.package_record.name.as_source()),
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
                name: format!("{} (host)", self.source.package_record.name.as_source()),
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
                record: self.source,
                method: BackendSourceBuildMethod::BuildV2(BackendSourceBuildV2Method {
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
    pub fn new(working_directory: &Path, host_platform: Platform) -> Self {
        let build_prefix = working_directory.join("bld");
        let host_prefix = if host_platform.is_windows() {
            working_directory.join("host")
        } else {
            let placeholder_template = "_placehold";
            let mut placeholder = String::new();
            let placeholder_length: usize = 255;

            while placeholder.len() < placeholder_length {
                placeholder.push_str(placeholder_template);
            }

            let placeholder = placeholder
                [0..placeholder_length - working_directory.join("host_env").as_os_str().len()]
                .to_string();

            working_directory.join(format!("host_env{}", placeholder))
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

    #[error("backend returned a dependency on an invalid package name: {0}")]
    InvalidPackageName(String, #[source] InvalidPackageNameError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendBuildError(#[from] BackendSourceBuildError),
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
