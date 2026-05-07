//! Per-key reporter traits.

use std::collections::BTreeMap;

use futures::Stream;
use pixi_build_discovery::JsonRpcBackendSpec;
use pixi_compute_reporters::OperationId;
use pixi_git::resolver::RepositoryReference;
use pixi_spec::{PixiSpec, ResolvedExcludeNewer};
use pixi_variant::VariantValue;
use rattler_conda_types::PackageName;
use serde::Serialize;
use url::Url;

use crate::{
    BackendSourceBuildSpec, BuildBackendMetadataInner, BuildBackendMetadataSpec,
    SolveCondaEnvironmentSpec, install_pixi::InstallPixiEnvironmentSpec,
};

/// Reporter-facing view for one source package's metadata resolution.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMetadataReporterSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,

    /// The timestamp exclusion to apply when retrieving the metadata.
    pub exclude_newer: Option<ResolvedExcludeNewer>,
}

/// Reporter-facing view for one variant's source-record assembly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceRecordReporterSpec {
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

pub trait PixiInstallReporter: Send + Sync {
    fn on_queued(&self, env: &InstallPixiEnvironmentSpec) -> OperationId;
    fn on_started(&self, install_id: OperationId);
    fn on_finished(&self, install_id: OperationId);

    /// Build a per-call rattler install reporter that nests under this
    /// install. `None` skips install-progress reporting.
    fn create_install_reporter(&self) -> Option<Box<dyn rattler::install::Reporter>> {
        None
    }
}

/// Lightweight reporter-facing view of a pixi environment solve.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PixiSolveEnvironmentSpec {
    pub name: String,
    pub platform: rattler_conda_types::Platform,
    /// True if the environment has direct binary URL/path conda deps,
    /// which trigger package-cache validation during the solve.
    pub has_direct_conda_dependency: bool,
}

pub trait PixiSolveReporter: Send + Sync {
    fn on_queued(&self, env: &PixiSolveEnvironmentSpec) -> OperationId;
    fn on_started(&self, solve_id: OperationId);
    fn on_finished(&self, solve_id: OperationId);
}

pub trait CondaSolveReporter: Send + Sync {
    fn on_queued(&self, env: &SolveCondaEnvironmentSpec) -> OperationId;
    fn on_started(&self, solve_id: OperationId);
    fn on_finished(&self, solve_id: OperationId);
}

pub trait GitCheckoutReporter: Send + Sync {
    fn on_queued(&self, env: &RepositoryReference) -> OperationId;
    fn on_started(&self, checkout_id: OperationId);
    fn on_finished(&self, checkout_id: OperationId);
}

pub trait UrlCheckoutReporter: Send + Sync {
    fn on_queued(&self, env: &Url) -> OperationId;
    fn on_started(&self, checkout_id: OperationId);
    fn on_finished(&self, checkout_id: OperationId);
}

/// Reporter for the compute-engine [`InstantiateBackendKey`](crate::InstantiateBackendKey).
pub trait InstantiateBackendReporter: Send + Sync {
    fn on_queued(&self, spec: &JsonRpcBackendSpec) -> OperationId;
    fn on_started(&self, id: OperationId);
    fn on_finished(&self, id: OperationId);

    /// Build a per-call rattler install reporter for the ephemeral
    /// build-tool prefix populated as part of this backend instantiation.
    fn create_install_reporter(&self) -> Option<Box<dyn rattler::install::Reporter>> {
        None
    }
}

pub trait BuildBackendMetadataReporter: Send + Sync {
    fn on_queued(&self, env: &BuildBackendMetadataInner) -> OperationId;

    /// `backend_output_stream` carries the backend's stdout/stderr so the
    /// reporter can stream it as it arrives.
    fn on_started(
        &self,
        id: OperationId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    fn on_finished(&self, id: OperationId, failed: bool);
}

pub trait SourceRecordReporter: Send + Sync {
    fn on_queued(&self, spec: &SourceRecordReporterSpec) -> OperationId;
    fn on_started(&self, id: OperationId);
    fn on_finished(&self, id: OperationId);
}

pub trait SourceMetadataReporter: Send + Sync {
    fn on_queued(&self, spec: &SourceMetadataReporterSpec) -> OperationId;
    fn on_started(&self, id: OperationId);
    fn on_finished(&self, id: OperationId);
}

pub trait BackendSourceBuildReporter: Send + Sync {
    fn on_queued(&self, env: &BackendSourceBuildSpec) -> OperationId;

    /// `backend_output_stream` carries the backend's stdout/stderr so the
    /// reporter can stream it as it arrives.
    fn on_started(
        &self,
        id: OperationId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    fn on_finished(&self, id: OperationId, failed: bool);
}

/// Returns whether the environment has direct binary URL/path dependencies.
///
/// The top-level progress reporter uses this to initialize rayon through uv's
/// initialization path before the solve later validates those direct binary
/// package-cache entries in parallel.
pub(crate) fn has_direct_conda_dependency(
    dependencies: &pixi_spec_containers::DependencyMap<rattler_conda_types::PackageName, PixiSpec>,
) -> bool {
    dependencies.iter_specs().any(|(_, spec)| match spec {
        PixiSpec::Url(url) => url.is_binary(),
        PixiSpec::Path(path) => path.is_binary(),
        _ => false,
    })
}
