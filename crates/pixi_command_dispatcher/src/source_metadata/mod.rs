pub(crate) mod cycle;

use std::sync::Arc;

use crate::{
    BuildBackendMetadataError, BuildBackendMetadataSpec, PackageNotProvidedError,
    build::PinnedSourceCodeLocation,
};
pub use cycle::{Cycle, CycleEnvironment};
use miette::Diagnostic;
use pixi_record::SourceRecord;
use pixi_spec::ResolvedExcludeNewer;
use rattler_conda_types::PackageName;
use thiserror::Error;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMetadataSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,

    /// The timestamp exclusion to apply when retrieving the metadata.
    pub exclude_newer: Option<ResolvedExcludeNewer>,
}

/// The result of resolving source metadata for all variants of a package.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceMetadata {
    /// Manifest and optional build source location for this metadata.
    pub source: PinnedSourceCodeLocation,

    /// The metadata that was acquired from the build backend.
    pub records: Vec<Arc<SourceRecord>>,
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceRecord(#[from] crate::source_record::SourceRecordError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] crate::SourceCheckoutError),
}
