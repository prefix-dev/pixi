use std::{collections::BTreeMap, str::FromStr};

use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::procedures::conda_outputs::{CondaOutput, CondaOutputDependencies};
use pixi_record::PinnedSourceSpec;
use pixi_spec::{BinarySpec, PixiSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{ChannelConfig, ChannelUrl, InvalidPackageNameError, PackageName};
use thiserror::Error;
use tracing::instrument;

use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher,
    CommandDispatcherError, CommandDispatcherErrorResultExt,
    build::{conversion, source_metadata_cache::MetadataKind},
};

/// A specification for retrieving the dependencies of a specific output from a
/// source package.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct GetOutputDependenciesSpec {
    /// The source specification. This should be a pinned source (e.g., a
    /// specific git commit) to ensure reproducibility.
    pub source: PinnedSourceSpec,

    /// The name of the output to retrieve dependencies for.
    pub output_name: PackageName,

    /// The channel configuration to use for the build backend.
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about the build environment.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The protocols that are enabled for this source
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

/// The dependencies of a specific output from a source package.
#[derive(Debug, Clone)]
pub struct OutputDependencies {
    /// The build dependencies of the package. These refer to the packages that
    /// should be installed in the "build" environment.
    pub build_dependencies: Option<DependencyMap<PackageName, PixiSpec>>,

    /// Additional constraints for the build environment.
    pub build_constraints: Option<DependencyMap<PackageName, BinarySpec>>,

    /// The "host" dependencies of the package. These refer to the packages that
    /// should be installed to be able to refer to them from the build process
    /// but not run them.
    pub host_dependencies: Option<DependencyMap<PackageName, PixiSpec>>,

    /// Additional constraints for the host environment.
    pub host_constraints: Option<DependencyMap<PackageName, BinarySpec>>,

    /// The dependencies for the run environment of the package.
    pub run_dependencies: DependencyMap<PackageName, PixiSpec>,

    /// Additional constraints for the run environment.
    pub run_constraints: DependencyMap<PackageName, BinarySpec>,
}

impl GetOutputDependenciesSpec {
    #[instrument(
        skip_all,
        name = "dev-sources",
        fields(
            source = %self.source,
            output = %self.output_name.as_source(),
            platform = %self.build_environment.host_platform,
        )
    )]
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<OutputDependencies, CommandDispatcherError<GetOutputDependenciesError>> {
        // Get the metadata from the build backend.
        let backend_metadata_spec = BuildBackendMetadataSpec {
            source: self.source.clone(),
            channel_config: self.channel_config,
            channels: self.channels,
            build_environment: self.build_environment,
            variants: self.variants,
            enabled_protocols: self.enabled_protocols,
        };

        let build_backend_metadata = command_dispatcher
            .build_backend_metadata(backend_metadata_spec)
            .await
            .map_err_with(GetOutputDependenciesError::BuildBackendMetadata)?;

        // Extract the outputs from the metadata.
        let outputs = match &build_backend_metadata.metadata.metadata {
            MetadataKind::Outputs { outputs } => outputs,
            MetadataKind::GetMetadata { .. } => {
                return Err(CommandDispatcherError::Failed(
                    GetOutputDependenciesError::UnsupportedProtocol,
                ));
            }
        };

        // Find the output with the matching name.
        let output = outputs
            .iter()
            .find(|output| output.metadata.name == self.output_name)
            .ok_or_else(|| {
                CommandDispatcherError::Failed(GetOutputDependenciesError::OutputNotFound {
                    output_name: self.output_name.clone(),
                    available_outputs: outputs.iter().map(|o| o.metadata.name.clone()).collect(),
                })
            })?;

        // Extract and return the dependencies.
        extract_dependencies(output).map_err(CommandDispatcherError::Failed)
    }
}

/// Extracts the dependencies from a CondaOutput and converts them to PixiSpecs.
fn extract_dependencies(
    output: &CondaOutput,
) -> Result<OutputDependencies, GetOutputDependenciesError> {
    let (build_deps, build_constraints) = output
        .build_dependencies
        .as_ref()
        .map(convert_conda_dependencies)
        .transpose()?
        .map(|(deps, constraints)| (Some(deps), Some(constraints)))
        .unwrap_or((None, None));

    let (host_deps, host_constraints) = output
        .host_dependencies
        .as_ref()
        .map(convert_conda_dependencies)
        .transpose()?
        .map(|(deps, constraints)| (Some(deps), Some(constraints)))
        .unwrap_or((None, None));

    let (run_deps, run_constraints) = convert_conda_dependencies(&output.run_dependencies)?;

    Ok(OutputDependencies {
        build_dependencies: build_deps,
        build_constraints,
        host_dependencies: host_deps,
        host_constraints,
        run_dependencies: run_deps,
        run_constraints,
    })
}

/// Converts CondaOutputDependencies to DependencyMaps of PixiSpecs and BinarySpecs.
fn convert_conda_dependencies(
    deps: &CondaOutputDependencies,
) -> Result<
    (
        DependencyMap<PackageName, PixiSpec>,
        DependencyMap<PackageName, BinarySpec>,
    ),
    GetOutputDependenciesError,
> {
    let mut dependencies = DependencyMap::default();
    let mut constraints = DependencyMap::default();

    // Convert depends
    for depend in &deps.depends {
        let name = PackageName::from_str(&depend.name).map_err(|err| {
            GetOutputDependenciesError::InvalidPackageName(depend.name.clone(), err)
        })?;

        let spec = conversion::from_package_spec_v1(depend.spec.clone());
        dependencies.insert(name, spec);
    }

    // Convert constraints
    for constraint in &deps.constraints {
        let name = PackageName::from_str(&constraint.name).map_err(|err| {
            GetOutputDependenciesError::InvalidPackageName(constraint.name.clone(), err)
        })?;

        let spec = conversion::from_binary_spec_v1(constraint.spec.clone());
        constraints.insert(name, spec);
    }

    Ok((dependencies, constraints))
}

#[derive(Debug, Error, Diagnostic)]
pub enum GetOutputDependenciesError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error(
        "the build backend does not support the `conda/outputs` procedure, which is required to retrieve output-specific dependencies"
    )]
    UnsupportedProtocol,

    #[error(
        "the output '{}' was not found in the source package. Available outputs: {}",
        output_name.as_source(),
        available_outputs.iter().map(|n| n.as_source()).collect::<Vec<_>>().join(", ")
    )]
    OutputNotFound {
        output_name: PackageName,
        available_outputs: Vec<PackageName>,
    },

    #[error("backend returned a dependency on an invalid package name: {0}")]
    InvalidPackageName(String, #[source] InvalidPackageNameError),
}
