use std::collections::BTreeMap;

use miette::Diagnostic;
use pixi_record::{DevSourceRecord, PinnedSourceSpec};
use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;
use thiserror::Error;
use tracing::instrument;

use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, CommandDispatcher, CommandDispatcherError,
    CommandDispatcherErrorResultExt, build::source_metadata_cache::MetadataKind,
};

/// A specification for retrieving development source metadata.
///
/// This queries the build backend for all outputs from a source and creates
/// DevSourceRecords for each one.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct DevSourceMetadataSpec {
    /// The development source specification
    pub package_name: PackageName,

    /// Information about the build backend to request the information from
    pub backend_metadata: BuildBackendMetadataSpec,
}

/// The result of querying development source metadata.
#[derive(Debug, Clone)]
pub struct DevSourceMetadata {
    /// Information about the source checkout that was used
    pub source: PinnedSourceSpec,

    /// All the dev source records for outputs from this source
    pub records: Vec<DevSourceRecord>,
}

/// An error that can occur while retrieving dev source metadata.
#[derive(Debug, Error, Diagnostic)]
pub enum DevSourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error(
        "the build backend does not support the `conda/outputs` procedure, which is required for development sources"
    )]
    UnsupportedProtocol,
}

impl DevSourceMetadataSpec {
    /// Retrieves development source metadata by querying the build backend.
    ///
    /// This method:
    /// 1. Gets metadata from the build backend
    /// 2. Creates a DevSourceRecord for each output
    /// 3. Combines build/host/run dependencies for each output
    #[instrument(
        skip_all,
        name = "dev-source-metadata",
        fields(
            source = %self.backend_metadata.source,
            platform = %self.backend_metadata.build_environment.host_platform,
        )
    )]
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<DevSourceMetadata, CommandDispatcherError<DevSourceMetadataError>> {
        // Get the metadata from the build backend
        let build_backend_metadata = command_dispatcher
            .build_backend_metadata(self.backend_metadata.clone())
            .await
            .map_err_with(DevSourceMetadataError::BuildBackendMetadata)?;

        // We only support the Outputs protocol for dev sources
        let outputs = match &build_backend_metadata.metadata.metadata {
            MetadataKind::Outputs { outputs } => outputs,
            MetadataKind::GetMetadata { .. } => {
                return Err(CommandDispatcherError::Failed(
                    DevSourceMetadataError::UnsupportedProtocol,
                ));
            }
        };

        // Create a SourceAnchor for resolving relative paths in dependencies
        let source_anchor = SourceAnchor::from(pixi_spec::SourceSpec::from(
            build_backend_metadata.source.clone(),
        ));

        // Create a DevSourceRecord for each output
        let mut records = Vec::new();
        for output in outputs {
            if output.metadata.name != self.package_name {
                continue;
            }
            let record = Self::create_dev_source_record(
                output,
                &build_backend_metadata.source,
                &build_backend_metadata.metadata.input_hash,
                &self.backend_metadata.variants,
                &source_anchor,
            )?;
            records.push(record);
        }

        Ok(DevSourceMetadata {
            source: build_backend_metadata.source.clone(),
            records,
        })
    }

    /// Creates a DevSourceRecord from a CondaOutput.
    ///
    /// This combines all dependencies (build, host, run) into a single map
    /// and resolves relative source paths.
    fn create_dev_source_record(
        output: &pixi_build_types::procedures::conda_outputs::CondaOutput,
        source: &PinnedSourceSpec,
        input_hash: &Option<pixi_record::InputHash>,
        variants: &Option<BTreeMap<String, Vec<String>>>,
        source_anchor: &SourceAnchor,
    ) -> Result<DevSourceRecord, CommandDispatcherError<DevSourceMetadataError>> {
        // Combine all dependencies into a single map
        let mut all_dependencies = DependencyMap::default();
        let mut all_constraints = DependencyMap::default();

        // Helper to process dependencies and resolve paths
        let process_deps =
            |deps: Option<
                &pixi_build_types::procedures::conda_outputs::CondaOutputDependencies,
            >,
             dependencies: &mut DependencyMap<PackageName, PixiSpec>,
             constraints: &mut DependencyMap<PackageName, BinarySpec>| {
                if let Some(deps) = deps {
                    // Process depends
                    for depend in &deps.depends {
                        let name = PackageName::new_unchecked(&depend.name);
                        let spec =
                            crate::build::conversion::from_package_spec_v1(depend.spec.clone());

                        // Resolve relative paths for source dependencies
                        let resolved_spec = match spec.into_source_or_binary() {
                            itertools::Either::Left(source_spec) => {
                                PixiSpec::from(source_anchor.resolve(source_spec))
                            }
                            itertools::Either::Right(binary_spec) => PixiSpec::from(binary_spec),
                        };
                        dependencies.insert(name, resolved_spec);
                    }

                    // Process constraints
                    for constraint in &deps.constraints {
                        let name = PackageName::new_unchecked(&constraint.name);
                        let spec =
                            crate::build::conversion::from_binary_spec_v1(constraint.spec.clone());
                        constraints.insert(name, spec);
                    }
                }
            };

        // Process all dependency types
        process_deps(
            output.build_dependencies.as_ref(),
            &mut all_dependencies,
            &mut all_constraints,
        );
        process_deps(
            output.host_dependencies.as_ref(),
            &mut all_dependencies,
            &mut all_constraints,
        );
        process_deps(
            Some(&output.run_dependencies),
            &mut all_dependencies,
            &mut all_constraints,
        );

        // Extract variant values (not the lists of possible values)
        // For now, we'll take the first value of each variant
        // TODO: This needs to be properly handled based on the actual variant selection
        let variant_values = variants
            .as_ref()
            .map(|v| {
                v.iter()
                    .filter_map(|(k, values)| {
                        values.first().map(|first| (k.clone(), first.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(DevSourceRecord {
            name: output.metadata.name.clone(),
            source: source.clone(),
            input_hash: input_hash.clone(),
            variants: variant_values,
            dependencies: all_dependencies,
            constraints: all_constraints,
        })
    }
}
