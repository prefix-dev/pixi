use std::{collections::BTreeMap, sync::Arc};

use futures::Stream;
use pixi_build_discovery::JsonRpcBackendSpec;
use pixi_git::resolver::RepositoryReference;
use pixi_spec::{PixiSpec, ResolvedExcludeNewer};
use pixi_variant::VariantValue;
use rattler_conda_types::PackageName;
use rattler_repodata_gateway::RunExportsReporter;
use serde::Serialize;
use url::Url;

use crate::{
    BackendSourceBuildSpec, BuildBackendMetadataInner, BuildBackendMetadataSpec,
    SolveCondaEnvironmentSpec, install_pixi::InstallPixiEnvironmentSpec,
};

/// Reporter-facing view of the in-flight metadata resolution for a single
/// source package. Constructed inside
/// [`ResolveSourcePackageKey`](crate::keys::ResolveSourcePackageKey) just
/// to feed the [`SourceMetadataReporter`] lifecycle; the Key itself
/// drives the work via [`SourceMetadataSpec`](crate::keys::SourceMetadataSpec).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMetadataReporterSpec {
    /// The name of the package to retrieve metadata from.
    pub package: PackageName,

    /// Information about the build backend to request the information from.
    pub backend_metadata: BuildBackendMetadataSpec,

    /// The timestamp exclusion to apply when retrieving the metadata.
    pub exclude_newer: Option<ResolvedExcludeNewer>,
}

/// Reporter-facing view of the in-flight resolution for a single source
/// record (one variant of a source package). Built inside
/// [`assemble_source_record`](crate::keys::resolve_source_record) for
/// the [`SourceRecordReporter`] lifecycle.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PixiInstallId(pub usize);

pub trait PixiInstallReporter {
    /// Called when the [`crate::CommandDispatcher`] learns of a new pixi
    /// environment to install.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular installation. Other functions in this trait will use this
    /// identifier to link the events to the particular solve.
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId;

    /// Called when installation of the specified environment has started.
    fn on_started(&self, solve_id: PixiInstallId);

    /// Called when solving of the specified environment has finished.
    fn on_finished(&self, solve_id: PixiInstallId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PixiSolveId(pub usize);

/// Lightweight reporter-facing view of a pixi environment solve.
///
/// This intentionally carries only the fields used by solve reporters,
/// so compute-engine call sites don't need to clone full solve specs
/// just to emit lifecycle events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PixiSolveEnvironmentSpec {
    pub name: String,
    pub platform: rattler_conda_types::Platform,
    /// Direct binary URL/path dependencies trigger package-cache
    /// validation in the solve flow; that validation uses rayon, so
    /// reporters use this bit to eagerly initialize rayon through uv's
    /// initialization path before the validation work starts.
    pub has_direct_conda_dependency: bool,
}

pub trait PixiSolveReporter: Send + Sync {
    /// Called when the [`crate::CommandDispatcher`] learns of a new pixi
    /// environment to solve.
    ///
    /// The command_dispatcher might not immediately start solving the
    /// environment, there is a limit on the number of active solves to
    /// avoid starving the CPU and memory.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve. Other functions in this trait will use this identifier
    /// to link the events to the particular solve.
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        env: &PixiSolveEnvironmentSpec,
    ) -> PixiSolveId;

    /// Called when solving of the specified environment has started.
    fn on_started(&self, solve_id: PixiSolveId);

    /// Called when solving of the specified environment has finished.
    fn on_finished(&self, solve_id: PixiSolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct CondaSolveId(pub usize);

pub trait CondaSolveReporter: Send + Sync {
    /// Called when the [`crate::CommandDispatcher`] learns of a new conda
    /// environment to solve.
    ///
    /// The command_dispatcher might not immediately start solving the
    /// environment, there is a limit on the number of active solves to
    /// avoid starving the CPU and memory.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve. Other functions in this trait will use this identifier
    /// to link the events to the particular solve.
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        env: &SolveCondaEnvironmentSpec,
    ) -> CondaSolveId;

    /// Called when solving of the specified environment has started.
    fn on_started(&self, solve_id: CondaSolveId);

    /// Called when solving of the specified environment has finished.
    fn on_finished(&self, solve_id: CondaSolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct GitCheckoutId(pub usize);

pub trait GitCheckoutReporter: Send + Sync {
    /// Called when a git checkout was queued on the
    /// [`crate::CommandDispatcher`].
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        env: &RepositoryReference,
    ) -> GitCheckoutId;

    /// Called when the git checkout has started.
    fn on_started(&self, checkout_id: GitCheckoutId);

    /// Called when the git checkout has finished.
    fn on_finished(&self, checkout_id: GitCheckoutId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct UrlCheckoutId(pub usize);

pub trait UrlCheckoutReporter: Send + Sync {
    /// Called when a url checkout was queued on the
    /// [`crate::CommandDispatcher`].
    fn on_queued(&self, reason: Option<ReporterContext>, env: &Url) -> UrlCheckoutId;

    /// Called when the url checkout has started.
    fn on_started(&self, checkout_id: UrlCheckoutId);

    /// Called when the url checkout has finished.
    fn on_finished(&self, checkout_id: UrlCheckoutId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct InstantiateBackendId(pub usize);

/// Reporter for the compute-engine [`InstantiateBackendKey`](crate::InstantiateBackendKey).
///
/// Fires once per backend instantiation request, after the backend has
/// been discovered and its spec resolved against any active
/// [`BackendOverride`](pixi_build_frontend::BackendOverride). Child
/// operations performed by the instantiation (conda solves, the binary
/// install into the ephemeral prefix, etc.) attribute up to the
/// returned [`InstantiateBackendId`] via the task-local reporter
/// context.
pub trait InstantiateBackendReporter: Send + Sync {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        spec: &JsonRpcBackendSpec,
    ) -> InstantiateBackendId;

    /// Called when the operation has started.
    fn on_started(&self, id: InstantiateBackendId);

    /// Called when the operation has finished.
    fn on_finished(&self, id: InstantiateBackendId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct BuildBackendMetadataId(pub usize);

pub trait BuildBackendMetadataReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        env: &BuildBackendMetadataInner,
    ) -> BuildBackendMetadataId;

    /// Called when the operation has started.
    fn on_started(
        &self,
        id: BuildBackendMetadataId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    /// Called when the operation has finished.
    fn on_finished(&self, id: BuildBackendMetadataId, failed: bool);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct SourceRecordId(pub usize);

pub trait SourceRecordReporter: Send + Sync {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(&self, reason: Option<ReporterContext>, spec: &SourceRecordReporterSpec)
    -> SourceRecordId;

    /// Called when the operation has started.
    fn on_started(&self, id: SourceRecordId);

    /// Called when the operation has finished.
    fn on_finished(&self, id: SourceRecordId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct SourceMetadataId(pub usize);

pub trait SourceMetadataReporter: Send + Sync {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        spec: &SourceMetadataReporterSpec,
    ) -> SourceMetadataId;

    /// Called when the operation has started.
    fn on_started(&self, id: SourceMetadataId);

    /// Called when the operation has finished.
    fn on_finished(&self, id: SourceMetadataId);
}

/// A trait that is used to report the progress of a source build performed by
/// the [`crate::CommandDispatcher`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct BackendSourceBuildId(pub usize);

pub trait BackendSourceBuildReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &self,
        reason: Option<ReporterContext>,
        env: &BackendSourceBuildSpec,
    ) -> BackendSourceBuildId;

    /// Called when the operation has started. The `backend_output_stream`
    /// stream can be used to capture the output of the build process.
    fn on_started(
        &self,
        id: BackendSourceBuildId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    /// Called when the operation has finished.
    fn on_finished(&self, id: BackendSourceBuildId, failed: bool);
}

/// A trait that is used to report the progress of the
/// [`crate::CommandDispatcher`].
///
/// The reporter has to be `Send + Sync`.
pub trait Reporter: Send + Sync {
    /// Called when the command dispatcher thread starts.
    fn on_start(&self) {}

    /// Called to clear the current progress.
    fn on_clear(&self) {}

    /// Called when the command dispatcher thread is about to close.
    fn on_finished(&self) {}

    /// Returns a reference to a reporter that reports on any git
    /// progress.
    fn as_git_reporter(&self) -> Option<&dyn GitCheckoutReporter> {
        None
    }
    /// Returns a reference to a reporter that reports on any git
    /// progress.
    fn as_url_reporter(&self) -> Option<&dyn UrlCheckoutReporter> {
        None
    }
    /// Returns a reference to a reporter that reports on conda solve
    /// progress.
    fn as_conda_solve_reporter(&self) -> Option<&dyn CondaSolveReporter> {
        None
    }
    /// Returns a reference to a reporter that reports on an entire pixi
    /// solve progress. so that can mean solves for multiple ecosystems for
    /// an environment.
    fn as_pixi_solve_reporter(&self) -> Option<&dyn PixiSolveReporter> {
        None
    }
    /// Returns a reference to a reporter that reports on the progress
    /// of actual package installation.
    fn as_pixi_install_reporter(&self) -> Option<&dyn PixiInstallReporter> {
        None
    }
    /// Returns a reference to a reporter that reports on the progress
    /// of instantiating a build backend (discovery, override resolution,
    /// ephemeral env, activation, JSON-RPC handshake).
    fn as_instantiate_backend_reporter(&self) -> Option<&dyn InstantiateBackendReporter> {
        None
    }

    /// Returns a reference to a reporter that reports on the progress
    /// of fetching build backend metadata.
    fn as_build_backend_metadata_reporter(&self) -> Option<&dyn BuildBackendMetadataReporter> {
        None
    }

    /// Returns a reference to a reporter that reports on the progress
    /// of resolving source metadata (all variants for a package).
    fn as_source_metadata_reporter(&self) -> Option<&dyn SourceMetadataReporter> {
        None
    }

    /// Returns a reference to a reporter that reports on the progress
    /// of resolving source records.
    fn as_source_record_reporter(&self) -> Option<&dyn SourceRecordReporter> {
        None
    }

    /// Returns a reporter that reports gateway progress.
    fn create_gateway_reporter(
        &self,
        _reason: Option<ReporterContext>,
    ) -> Option<Box<dyn rattler_repodata_gateway::Reporter>> {
        None
    }

    /// Returns a reporter that run exports fetching progress.
    fn create_run_exports_reporter(
        &self,
        _reason: Option<ReporterContext>,
    ) -> Option<Arc<dyn RunExportsReporter>> {
        None
    }

    /// Returns a reporter that reports installation progress.
    fn create_install_reporter(
        &self,
        _reason: Option<ReporterContext>,
    ) -> Option<Box<dyn rattler::install::Reporter>> {
        None
    }

    /// Returns a reference to a reporter that reports on the progress
    /// of a backend that is building source packages.
    fn as_backend_source_build_reporter(&self) -> Option<&dyn BackendSourceBuildReporter> {
        None
    }
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

#[derive(Debug, Clone, Copy, Serialize, derive_more::From)]
#[serde(rename_all = "kebab-case")]
pub enum ReporterContext {
    SolvePixi(PixiSolveId),
    SolveConda(CondaSolveId),
    InstallPixi(PixiInstallId),
    SourceMetadata(SourceMetadataId),
    SourceRecord(SourceRecordId),
    BuildBackendMetadata(BuildBackendMetadataId),
    InstantiateBackend(InstantiateBackendId),
    BackendSourceBuild(BackendSourceBuildId),
}
