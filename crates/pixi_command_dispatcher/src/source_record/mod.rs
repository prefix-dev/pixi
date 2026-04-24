use std::{collections::BTreeMap, sync::Arc};

use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, PackageNotProvidedError,
    SolvePixiEnvironmentError,
    build::{DependenciesError, PinnedSourceCodeLocation},
    source_metadata::cycle::Cycle,
};
use miette::Diagnostic;
use pixi_record::SourceRecord;
use pixi_spec::{ResolvedExcludeNewer, SourceLocationSpec, SpecConversionError};
use pixi_variant::VariantValue;
use rattler_conda_types::{InvalidPackageNameError, PackageName};
use rattler_repodata_gateway::RunExportExtractorError;
use thiserror::Error;

/// A request for the resolved metadata of a single source record, identified
/// by package name and variant combination.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecordSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// The specific variant that identifies which build output to resolve.
    pub variants: BTreeMap<String, VariantValue>,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,

    /// Exclude packages newer than this cutoff when resolving build/host
    /// dependencies. Typically derived from locked source timestamps.
    pub exclude_newer: Option<ResolvedExcludeNewer>,
}

/// The result of resolving a single source record.
#[derive(Debug)]
pub struct ResolvedSourceRecord {
    /// Manifest and optional build source location for this record.
    pub source: PinnedSourceCodeLocation,

    /// The resolved source record.
    pub record: Arc<SourceRecord>,
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceRecordError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error("failed to amend run exports for {0} environment")]
    RunExportsExtraction(String, #[source] Arc<RunExportExtractorError>),

    #[error("while trying to solve the build environment for the package")]
    SolveBuildEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error("while trying to solve the host environment for the package")]
    SolveHostEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error(transparent)]
    SpecConversionError(Arc<SpecConversionError>),

    #[error(transparent)]
    InvalidPackageName(Arc<InvalidPackageNameError>),

    #[error(transparent)]
    PinCompatibleError(#[from] crate::build::pin_compatible::PinCompatibleError),

    #[error("found two source dependencies for {} but for different sources ({source1} and {source2})", package.as_source()
    )]
    DuplicateSourceDependency {
        package: PackageName,
        source1: Box<SourceLocationSpec>,
        source2: Box<SourceLocationSpec>,
    },

    #[error("the dependencies of some packages in the environment form a cycle")]
    Cycle(Cycle),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),

    #[error(
        "no output with matching variants found for package '{package}' at '{manifest_path}', available outputs: {available}"
    )]
    NoMatchingVariant {
        package: String,
        manifest_path: String,
        available: String,
    },

    /// Pinning or checking out the source failed.
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] crate::SourceCheckoutError),
}

impl From<SpecConversionError> for SourceRecordError {
    fn from(err: SpecConversionError) -> Self {
        Self::SpecConversionError(Arc::new(err))
    }
}

impl From<InvalidPackageNameError> for SourceRecordError {
    fn from(err: InvalidPackageNameError) -> Self {
        Self::InvalidPackageName(Arc::new(err))
    }
}

impl From<DependenciesError> for SourceRecordError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(error) => {
                SourceRecordError::InvalidPackageName(error)
            }
            DependenciesError::PinCompatibleError(error) => {
                SourceRecordError::PinCompatibleError(error)
            }
        }
    }
}
