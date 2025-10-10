use std::collections::BTreeMap;

use miette::Diagnostic;
use pixi_build_discovery::EnabledProtocols;
use pixi_build_types::procedures::conda_outputs::{CondaOutput, CondaOutputDependencies};
use pixi_record::PinnedSourceSpec;
use rattler_conda_types::{ChannelConfig, ChannelUrl, PackageName};
use thiserror::Error;
use tracing::instrument;

use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, BuildEnvironment, CommandDispatcher,
    CommandDispatcherError, CommandDispatcherErrorResultExt,
    build::source_metadata_cache::MetadataKind,
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
    pub build_dependencies: Option<CondaOutputDependencies>,

    /// The "host" dependencies of the package. These refer to the packages that
    /// should be installed to be able to refer to them from the build process
    /// but not run them.
    pub host_dependencies: Option<CondaOutputDependencies>,

    /// The dependencies for the run environment of the package.
    pub run_dependencies: CondaOutputDependencies,
}

impl GetOutputDependenciesSpec {
    #[instrument(
        skip_all,
        name = "output-dependencies",
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
        Ok(extract_dependencies(output))
    }
}

/// Extracts the dependencies from a CondaOutput.
fn extract_dependencies(output: &CondaOutput) -> OutputDependencies {
    OutputDependencies {
        build_dependencies: output.build_dependencies.clone(),
        host_dependencies: output.host_dependencies.clone(),
        run_dependencies: output.run_dependencies.clone(),
    }
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
}
