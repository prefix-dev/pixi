use std::sync::Arc;

use futures::Stream;
use pixi_git::resolver::RepositoryReference;
use rattler_repodata_gateway::RunExportsReporter;
use serde::Serialize;

use crate::{
    BackendSourceBuildSpec, BuildBackendMetadataSpec, PixiEnvironmentSpec,
    SolveCondaEnvironmentSpec, SourceBuildSpec, SourceMetadataSpec,
    install_pixi::InstallPixiEnvironmentSpec, instantiate_tool_env::InstantiateToolEnvironmentSpec,
};

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
        &mut self,
        reason: Option<ReporterContext>,
        env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId;

    /// Called when solving of the specified environment has started.
    fn on_start(&mut self, solve_id: PixiInstallId);

    /// Called when solving of the specified environment has finished.
    fn on_finished(&mut self, solve_id: PixiInstallId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PixiSolveId(pub usize);

pub trait PixiSolveReporter {
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
        &mut self,
        reason: Option<ReporterContext>,
        env: &PixiEnvironmentSpec,
    ) -> PixiSolveId;

    /// Called when solving of the specified environment has started.
    fn on_start(&mut self, solve_id: PixiSolveId);

    /// Called when solving of the specified environment has finished.
    fn on_finished(&mut self, solve_id: PixiSolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct CondaSolveId(pub usize);

pub trait CondaSolveReporter {
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
        &mut self,
        reason: Option<ReporterContext>,
        env: &SolveCondaEnvironmentSpec,
    ) -> CondaSolveId;

    /// Called when solving of the specified environment has started.
    fn on_start(&mut self, solve_id: CondaSolveId);

    /// Called when solving of the specified environment has finished.
    fn on_finished(&mut self, solve_id: CondaSolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct GitCheckoutId(pub usize);

pub trait GitCheckoutReporter {
    /// Called when a git checkout was queued on the
    /// [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &RepositoryReference,
    ) -> GitCheckoutId;

    /// Called when the git checkout has started.
    fn on_start(&mut self, checkout_id: GitCheckoutId);

    /// Called when the git checkout has finished.
    fn on_finished(&mut self, checkout_id: GitCheckoutId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct InstantiateToolEnvId(pub usize);

pub trait InstantiateToolEnvironmentReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &InstantiateToolEnvironmentSpec,
    ) -> InstantiateToolEnvId;

    /// Called when the operation has started.
    fn on_started(&mut self, id: InstantiateToolEnvId);

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: InstantiateToolEnvId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct BuildBackendMetadataId(pub usize);

pub trait BuildBackendMetadataReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &BuildBackendMetadataSpec,
    ) -> BuildBackendMetadataId;

    /// Called when the operation has started.
    fn on_started(&mut self, id: BuildBackendMetadataId);

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: BuildBackendMetadataId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct SourceMetadataId(pub usize);

pub trait SourceMetadataReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &SourceMetadataSpec,
    ) -> SourceMetadataId;

    /// Called when the operation has started.
    fn on_started(&mut self, id: SourceMetadataId);

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: SourceMetadataId);
}

/// A trait that is used to report the progress of a source build performed by
/// the [`crate::CommandDispatcher`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct SourceBuildId(pub usize);

pub trait SourceBuildReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &SourceBuildSpec,
    ) -> SourceBuildId;

    /// Called when the operation has started.
    fn on_started(&mut self, id: SourceBuildId);

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: SourceBuildId);
}

/// A trait that is used to report the progress of a source build performed by
/// the [`crate::CommandDispatcher`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct BackendSourceBuildId(pub usize);

pub trait BackendSourceBuildReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &BackendSourceBuildSpec,
    ) -> BackendSourceBuildId;

    /// Called when the operation has started. The `backend_output_stream`
    /// stream can be used to capture the output of the build process.
    fn on_started(
        &mut self,
        id: BackendSourceBuildId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: BackendSourceBuildId, failed: bool);
}

/// A trait that is used to report the progress of the
/// [`crate::CommandDispatcher`].
///
/// The reporter has to be `Send` but does not require `Sync`.
pub trait Reporter: Send {
    /// Called when the command dispatcher thread starts.
    fn on_start(&mut self) {}

    /// Called to clear the current progress.
    fn on_clear(&mut self) {}

    /// Called when the command dispatcher thread is about to close.
    fn on_finished(&mut self) {}

    /// Returns a mutable reference to a reporter that reports on any git
    /// progress.
    fn as_git_reporter(&mut self) -> Option<&mut dyn GitCheckoutReporter> {
        None
    }
    /// Returns a mutable reference to a reporter that reports on conda solve
    /// progress.
    fn as_conda_solve_reporter(&mut self) -> Option<&mut dyn CondaSolveReporter> {
        None
    }
    /// Returns a mutable reference to a reporter that reports on an entire pixi
    /// solve progress. so that can mean solves for multiple ecosystems for
    /// an environment.
    fn as_pixi_solve_reporter(&mut self) -> Option<&mut dyn PixiSolveReporter> {
        None
    }
    /// Returns a mutable reference to a reporter that reports on the progress
    /// of actual package installation.
    fn as_pixi_install_reporter(&mut self) -> Option<&mut dyn PixiInstallReporter> {
        None
    }
    /// Returns a mutable reference to a reporter that reports on the progress
    /// of instantiating a tool environment.
    fn as_instantiate_tool_environment_reporter(
        &mut self,
    ) -> Option<&mut dyn InstantiateToolEnvironmentReporter> {
        None
    }

    /// Returns a mutable reference to a reporter that reports on the progress
    /// of fetching build backend metadata.
    fn as_build_backend_metadata_reporter(
        &mut self,
    ) -> Option<&mut dyn BuildBackendMetadataReporter> {
        None
    }

    /// Returns a mutable reference to a reporter that reports on the progress
    /// of fetching source metadata.
    fn as_source_metadata_reporter(&mut self) -> Option<&mut dyn SourceMetadataReporter> {
        None
    }

    /// Returns a reporter that reports gateway progress.
    fn create_gateway_reporter(
        &mut self,
        _reason: Option<ReporterContext>,
    ) -> Option<Box<dyn rattler_repodata_gateway::Reporter>> {
        None
    }

    /// Returns a reporter that run exports fetching progress.
    fn create_run_exports_reporter(
        &mut self,
        _reason: Option<ReporterContext>,
    ) -> Option<Arc<dyn RunExportsReporter>> {
        None
    }

    /// Returns a reporter that reports installation progress.
    fn create_install_reporter(
        &mut self,
        _reason: Option<ReporterContext>,
    ) -> Option<Box<dyn rattler::install::Reporter>> {
        None
    }

    /// Returns a mutable reference to a reporter that reports on the progress
    /// of building source packages.
    fn as_source_build_reporter(&mut self) -> Option<&mut dyn SourceBuildReporter> {
        None
    }

    /// Returns a mutable reference to a reporter that reports on the progress
    /// of a backend that is building source packages.
    fn as_backend_source_build_reporter(&mut self) -> Option<&mut dyn BackendSourceBuildReporter> {
        None
    }
}

#[derive(Debug, Clone, Copy, Serialize, derive_more::From)]
#[serde(rename_all = "kebab-case")]
pub enum ReporterContext {
    SolvePixi(PixiSolveId),
    SolveConda(CondaSolveId),
    InstallPixi(PixiInstallId),
    SourceMetadata(SourceMetadataId),
    BuildBackendMetadata(BuildBackendMetadataId),
    InstantiateToolEnv(InstantiateToolEnvId),
    SourceBuild(SourceBuildId),
    BackendSourceBuild(BackendSourceBuildId),
}
