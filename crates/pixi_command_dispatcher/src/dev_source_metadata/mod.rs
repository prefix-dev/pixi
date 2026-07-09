use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;

use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_types::{ConstraintSpec, PackageSpec};
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_record::{DevSourceRecord, PinnedSourceSpec};
use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;
use thiserror::Error;
use tracing::instrument;

use crate::build::conversion;
use crate::{BuildBackendMetadataError, BuildBackendMetadataKey, BuildBackendMetadataSpec};

/// A specification for retrieving dev source metadata.
///
/// This queries the build backend for all outputs from a source and creates
/// DevSourceRecords for each one.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct DevSourceMetadataSpec {
    /// The dev source specification
    pub package_name: PackageName,

    /// The extra dependency groups of the package to include in the
    /// records' dependencies, in addition to the regular
    /// build/host/run dependencies.
    pub extras: Option<Vec<String>>,

    /// Information about the build backend to request the information from
    pub backend_metadata: BuildBackendMetadataSpec,
}

/// The result of querying dev source metadata.
#[derive(Debug, Clone)]
pub struct DevSourceMetadata {
    /// Information about the source checkout that was used
    pub source: PinnedSourceSpec,

    /// All the dev source records for outputs from this source
    pub records: Vec<DevSourceRecord>,
}

/// An error that can occur while retrieving dev source metadata.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum DevSourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] Box<BuildBackendMetadataError>),

    #[error(
        "the build backend does not support the `conda/outputs` procedure, which is required for dev sources"
    )]
    UnsupportedProtocol,

    #[error("detected a cycle while trying to retrieve dev source metadata")]
    Cycle,

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ExtraGroupNotProvided(#[from] ExtraGroupNotProvidedError),
}

/// Error for when a requested extra dependency group is not declared by the
/// package.
#[derive(Debug, Clone, Error)]
pub struct ExtraGroupNotProvidedError {
    /// The name of the package the extra was requested for
    pub name: PackageName,
    /// The extra group that was requested but not found
    pub extra: String,
    /// The extra groups the package does declare
    pub available_extras: Vec<String>,
}

impl Display for ExtraGroupNotProvidedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the package '{}' does not define the extra dependency group '{}'",
            self.name.as_source(),
            self.extra,
        )
    }
}

impl Diagnostic for ExtraGroupNotProvidedError {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        if self.available_extras.is_empty() {
            Some(Box::new(
                "the package does not define any extra dependency groups (`package.extra-dependencies`)".to_string(),
            ))
        } else {
            Some(Box::new(format!(
                "the package defines the following extra dependency groups: '{}'",
                self.available_extras.join("', '")
            )))
        }
    }
}

/// Error for when a package is not provided by the source.
#[derive(Debug, Clone, Error)]
pub struct PackageNotProvidedError {
    /// The name of the package that was requested
    pub name: PackageName,
    /// The pinned source specification
    pub pinned_source: Box<PinnedSourceSpec>,
    /// Similar package names that are provided by the source
    pub similar_names: Vec<String>,
}

impl PackageNotProvidedError {
    /// Creates a new `PackageNotProvidedError` with suggestions based on string similarity.
    pub fn new(
        name: PackageName,
        pinned_source: PinnedSourceSpec,
        available_names: impl IntoIterator<Item = PackageName>,
    ) -> Self {
        let name_str = name.as_source();
        let similar_names = available_names
            .into_iter()
            .filter_map(|available| {
                let distance = strsim::jaro(available.as_source(), name_str);
                (distance > 0.6).then_some((distance, available.as_source().to_string()))
            })
            .sorted_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, name)| name)
            .take(2)
            .collect();
        Self {
            name,
            pinned_source: Box::new(pinned_source),
            similar_names,
        }
    }
}

impl Display for PackageNotProvidedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the package '{}' is not provided by the project located at '{}'",
            self.name.as_source(),
            self.pinned_source
        )
    }
}

impl Diagnostic for PackageNotProvidedError {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        if !self.similar_names.is_empty() {
            Some(Box::new(format!(
                "Did you mean '{}'?",
                self.similar_names.join("' or '")
            )))
        } else {
            None
        }
    }
}

/// Compute-engine [`Key`] for dev source metadata.
///
/// Wraps the spec in an [`Arc`] so the key is cheap to hash/clone; the
/// Value is likewise [`Arc`]-wrapped so dedup fan-out shares the
/// record allocation.
#[derive(Clone, Debug, Hash, Eq, PartialEq, derive_more::Display)]
#[display("{}", _0.backend_metadata.manifest_source)]
pub struct DevSourceMetadataKey(pub Arc<DevSourceMetadataSpec>);

impl DevSourceMetadataKey {
    pub fn new(spec: DevSourceMetadataSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Key for DevSourceMetadataKey {
    type Value = Result<Arc<DevSourceMetadata>, DevSourceMetadataError>;

    #[instrument(
        skip_all,
        name = "dev-source-metadata",
        fields(
            source = %self.0.backend_metadata.manifest_source,
            platform = %self.0.backend_metadata.env_ref.display_platform(),
        )
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = &self.0;

        // Get the metadata from the build backend.
        let build_backend_metadata = ctx
            .compute(&BuildBackendMetadataKey::new(spec.backend_metadata.clone()))
            .await
            .map_err(|e| DevSourceMetadataError::BuildBackendMetadata(Box::new(e)))?;

        // Create a SourceAnchor for resolving relative paths in dependencies.
        let source_anchor = SourceAnchor::from(SourceSpec::from(
            build_backend_metadata.source.manifest_source().clone(),
        ));

        // Create a DevSourceRecord for each output that matches the requested package.
        let records: Vec<DevSourceRecord> = build_backend_metadata
            .metadata
            .outputs
            .iter()
            .filter(|output| output.metadata.name == spec.package_name)
            .map(|output| {
                DevSourceMetadataSpec::create_dev_source_record(
                    output,
                    spec.extras.as_deref(),
                    build_backend_metadata.source.manifest_source(),
                    &source_anchor,
                )
            })
            .collect::<Result<_, _>>()?;

        if records.is_empty() {
            let available_names = build_backend_metadata
                .metadata
                .outputs
                .iter()
                .map(|output| output.metadata.name.clone());
            return Err(PackageNotProvidedError::new(
                spec.package_name.clone(),
                build_backend_metadata.source.manifest_source().clone(),
                available_names,
            )
            .into());
        }

        Ok(Arc::new(DevSourceMetadata {
            source: build_backend_metadata.source.manifest_source().clone(),
            records,
        }))
    }
}

impl DevSourceMetadataSpec {
    /// Creates a DevSourceRecord from a CondaOutput.
    ///
    /// This combines all dependencies (build, host, run, and the requested
    /// extra groups) into a single map and resolves relative source paths.
    fn create_dev_source_record(
        output: &pixi_build_types::procedures::conda_outputs::CondaOutput,
        extras: Option<&[String]>,
        source: &PinnedSourceSpec,
        source_anchor: &SourceAnchor,
    ) -> Result<DevSourceRecord, ExtraGroupNotProvidedError> {
        // Combine all dependencies into a single map
        let mut all_dependencies = DependencyMap::default();
        let mut all_constraints = DependencyMap::default();

        // Helper to convert a single dependency spec, resolving relative
        // source paths. Returns `None` for pin-compatible dependencies:
        // since the dependencies for build and host are also added directly
        // the pin_compatible wouldn't have any effect anyway.
        let convert_spec = |spec: &PackageSpec| -> Option<PixiSpec> {
            match spec {
                PackageSpec::Binary(binary) => {
                    let spec = crate::build::conversion::from_binary_spec_v1((**binary).clone());
                    Some(PixiSpec::from(spec))
                }
                PackageSpec::Source(source) => {
                    let spec = crate::build::conversion::from_source_spec_v1(source.clone());
                    Some(PixiSpec::from(spec.resolve(source_anchor)))
                }
                PackageSpec::PinCompatible(_) => None,
            }
        };

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
                        let name = PackageName::new_unchecked(depend.name.as_str());
                        if let Some(resolved_spec) = convert_spec(&depend.spec) {
                            dependencies.insert(name, resolved_spec);
                        }
                    }

                    // Process constraints
                    for constraint in &deps.constraints {
                        let name = PackageName::new_unchecked(constraint.name.as_str());

                        // Match on ConstraintSpec enum
                        let spec = match &constraint.spec {
                            ConstraintSpec::Binary(binary) => {
                                conversion::from_binary_spec_v1(binary.clone())
                            }
                        };

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

        // Merge in the dependencies of the requested extra groups.
        for extra in extras.unwrap_or_default() {
            let Some(extra_deps) = output.extra_dependencies.get(extra.as_str()) else {
                return Err(ExtraGroupNotProvidedError {
                    name: output.metadata.name.clone(),
                    extra: extra.clone(),
                    available_extras: output
                        .extra_dependencies
                        .keys()
                        .map(ToString::to_string)
                        .collect(),
                });
            };
            for depend in extra_deps {
                let name = PackageName::new_unchecked(depend.name.as_str());
                if let Some(resolved_spec) = convert_spec(&depend.spec) {
                    all_dependencies.insert(name, resolved_spec);
                }
            }
        }

        // Use the variant values from the output metadata
        // The backend has already selected specific variant values for this output
        let variant_values = output.metadata.variant.clone();

        Ok(DevSourceRecord {
            name: output.metadata.name.clone(),
            source: source.clone(),
            variants: variant_values
                .clone()
                .into_iter()
                .map(|(k, v)| (k, pixi_variant::VariantValue::from(v)))
                .collect(),
            dependencies: all_dependencies,
            constraints: all_constraints,
        })
    }
}
