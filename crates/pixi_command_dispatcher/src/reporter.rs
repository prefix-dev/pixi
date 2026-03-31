use std::sync::Arc;

use futures::Stream;
use pixi_git::resolver::RepositoryReference;
use rattler_repodata_gateway::RunExportsReporter;
use serde::Serialize;
use url::Url;

use crate::{
    BackendSourceBuildSpec, BuildBackendMetadataSpec, PixiEnvironmentSpec,
    SolveCondaEnvironmentSpec, SourceBuildSpec, SourceMetadataSpec, SourceRecordSpec,
    install_pixi::InstallPixiEnvironmentSpec, instantiate_tool_env::InstantiateToolEnvironmentSpec,
};

/// An opaque identifier for a group of deduplicated tasks.
///
/// When multiple callers request the same computation, they share a single
/// execution. All callers in the same group receive the same `DedupGroupId`
/// so reporter implementations can correlate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct DedupGroupId(pub usize);

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

    /// Called when installation of the specified environment has started.
    fn on_started(&mut self, solve_id: PixiInstallId);

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
    fn on_started(&mut self, solve_id: PixiSolveId);

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
    fn on_started(&mut self, solve_id: CondaSolveId);

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
        dedup_id: DedupGroupId,
    ) -> GitCheckoutId;

    /// Called when the git checkout has started.
    fn on_started(&mut self, checkout_id: GitCheckoutId);

    /// Called when the git checkout has finished.
    fn on_finished(&mut self, checkout_id: GitCheckoutId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct UrlCheckoutId(pub usize);

pub trait UrlCheckoutReporter {
    /// Called when a url checkout was queued on the
    /// [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &Url,
        dedup_id: DedupGroupId,
    ) -> UrlCheckoutId;

    /// Called when the url checkout has started.
    fn on_started(&mut self, checkout_id: UrlCheckoutId);

    /// Called when the url checkout has finished.
    fn on_finished(&mut self, checkout_id: UrlCheckoutId);
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
        dedup_id: DedupGroupId,
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
        dedup_id: DedupGroupId,
    ) -> BuildBackendMetadataId;

    /// Called when the operation has started.
    fn on_started(
        &mut self,
        id: BuildBackendMetadataId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: BuildBackendMetadataId, failed: bool);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct SourceRecordId(pub usize);

pub trait SourceRecordReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        spec: &SourceRecordSpec,
        dedup_id: DedupGroupId,
    ) -> SourceRecordId;

    /// Called when the operation has started.
    fn on_started(&mut self, id: SourceRecordId);

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: SourceRecordId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct SourceMetadataId(pub usize);

pub trait SourceMetadataReporter {
    /// Called when an operation was queued on the [`crate::CommandDispatcher`].
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        spec: &SourceMetadataSpec,
        dedup_id: DedupGroupId,
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
        dedup_id: DedupGroupId,
    ) -> SourceBuildId;

    /// Called when the operation has started.
    fn on_started(
        &mut self,
        id: SourceBuildId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    );

    /// Called when the operation has finished.
    fn on_finished(&mut self, id: SourceBuildId, failed: bool);
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
    /// Returns a mutable reference to a reporter that reports on any git
    /// progress.
    fn as_url_reporter(&mut self) -> Option<&mut dyn UrlCheckoutReporter> {
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
    /// of resolving source metadata (all variants for a package).
    fn as_source_metadata_reporter(&mut self) -> Option<&mut dyn SourceMetadataReporter> {
        None
    }

    /// Returns a mutable reference to a reporter that reports on the progress
    /// of resolving source records.
    fn as_source_record_reporter(&mut self) -> Option<&mut dyn SourceRecordReporter> {
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

/// Trait for task specs whose lifecycle events can be reported through
/// [`Reporter`].
///
/// Each implementation bridges to the appropriate specific reporter trait
/// (e.g. `SourceBuildReporter`, `GitCheckoutReporter`) so that generic
/// handler code can notify the reporter without knowing which trait to use.
///
/// The `report_queued` method takes an `Option<DedupGroupId>`:
/// - `Some(id)` for deduplicated tasks (passed to reporters that accept it)
/// - `None` for non-deduplicated tasks (ignored by the implementation)
pub(crate) trait Reportable {
    /// The reporter ID type returned by `report_queued`.
    type ReporterId: Copy;

    /// Notify the reporter that this task was queued.
    ///
    /// `dedup_group_id` is `Some` for deduplicated tasks, `None` otherwise.
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId>;

    /// Notify the reporter that this task has started.
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId);

    /// Notify the reporter that this task has finished.
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        failed: bool,
    );
}

// --- Reportable implementations ---

macro_rules! impl_reportable {
    // Dedup reporter with on_started(id) and on_finished(id)
    ($spec:ty, $id:ty, $accessor:ident $(, queued_arg: $arg:expr)?) => {
        impl Reportable for $spec {
            type ReporterId = $id;
            fn report_queued(
                &self,
                reporter: &mut Option<Box<dyn Reporter>>,
                parent: Option<ReporterContext>,
                dedup_group_id: Option<DedupGroupId>,
            ) -> Option<Self::ReporterId> {
                reporter.as_deref_mut()
                    .and_then(|r| r.$accessor())
                    .map(|r| r.on_queued(parent, impl_reportable!(@queued_spec self $(, $arg)?), dedup_group_id.expect("dedup tasks must provide a DedupGroupId")))
            }
            fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
                if let Some(r) = reporter.as_deref_mut().and_then(|r| r.$accessor()) { r.on_started(id); }
            }
            fn report_finished(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId, _failed: bool) {
                if let Some(r) = reporter.as_deref_mut().and_then(|r| r.$accessor()) { r.on_finished(id); }
            }
        }
    };
    (@queued_spec $self:ident) => { $self };
    (@queued_spec $self:ident, $arg:expr) => { $arg };
}

impl_reportable!(
    InstantiateToolEnvironmentSpec,
    InstantiateToolEnvId,
    as_instantiate_tool_environment_reporter
);
impl_reportable!(SourceRecordSpec, SourceRecordId, as_source_record_reporter);
impl_reportable!(
    SourceMetadataSpec,
    SourceMetadataId,
    as_source_metadata_reporter
);

impl Reportable for pixi_git::GitUrl {
    type ReporterId = GitCheckoutId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        let repo_ref = pixi_git::resolver::RepositoryReference::from(self);
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_git_reporter())
            .map(|r| {
                r.on_queued(
                    parent,
                    &repo_ref,
                    dedup_group_id.expect("dedup tasks must provide a DedupGroupId"),
                )
            })
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter.as_deref_mut().and_then(|r| r.as_git_reporter()) {
            r.on_started(id);
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        _failed: bool,
    ) {
        if let Some(r) = reporter.as_deref_mut().and_then(|r| r.as_git_reporter()) {
            r.on_finished(id);
        }
    }
}

impl Reportable for pixi_spec::UrlSpec {
    type ReporterId = UrlCheckoutId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_url_reporter())
            .map(|r| {
                r.on_queued(
                    parent,
                    &self.url,
                    dedup_group_id.expect("dedup tasks must provide a DedupGroupId"),
                )
            })
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter.as_deref_mut().and_then(|r| r.as_url_reporter()) {
            r.on_started(id);
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        _failed: bool,
    ) {
        if let Some(r) = reporter.as_deref_mut().and_then(|r| r.as_url_reporter()) {
            r.on_finished(id);
        }
    }
}

impl Reportable for BuildBackendMetadataSpec {
    type ReporterId = BuildBackendMetadataId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_build_backend_metadata_reporter())
            .map(|r| {
                r.on_queued(
                    parent,
                    self,
                    dedup_group_id.expect("dedup tasks must provide a DedupGroupId"),
                )
            })
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_build_backend_metadata_reporter())
        {
            r.on_started(id, Box::new(futures::stream::empty()));
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        failed: bool,
    ) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_build_backend_metadata_reporter())
        {
            r.on_finished(id, failed);
        }
    }
}

impl Reportable for SourceBuildSpec {
    type ReporterId = SourceBuildId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_source_build_reporter())
            .map(|r| {
                r.on_queued(
                    parent,
                    self,
                    dedup_group_id.expect("dedup tasks must provide a DedupGroupId"),
                )
            })
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_source_build_reporter())
        {
            r.on_started(id, Box::new(futures::stream::empty()));
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        failed: bool,
    ) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_source_build_reporter())
        {
            r.on_finished(id, failed);
        }
    }
}

impl Reportable for crate::PixiEnvironmentSpec {
    type ReporterId = PixiSolveId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        _dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_pixi_solve_reporter())
            .map(|r| r.on_queued(parent, self))
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_pixi_solve_reporter())
        {
            r.on_started(id);
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        _failed: bool,
    ) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_pixi_solve_reporter())
        {
            r.on_finished(id);
        }
    }
}

impl Reportable for SolveCondaEnvironmentSpec {
    type ReporterId = CondaSolveId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        _dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_conda_solve_reporter())
            .map(|r| r.on_queued(parent, self))
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_conda_solve_reporter())
        {
            r.on_started(id);
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        _failed: bool,
    ) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_conda_solve_reporter())
        {
            r.on_finished(id);
        }
    }
}

impl Reportable for InstallPixiEnvironmentSpec {
    type ReporterId = PixiInstallId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        _dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_pixi_install_reporter())
            .map(|r| r.on_queued(parent, self))
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_pixi_install_reporter())
        {
            r.on_started(id);
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        _failed: bool,
    ) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_pixi_install_reporter())
        {
            r.on_finished(id);
        }
    }
}

impl Reportable for BackendSourceBuildSpec {
    type ReporterId = BackendSourceBuildId;
    fn report_queued(
        &self,
        reporter: &mut Option<Box<dyn Reporter>>,
        parent: Option<ReporterContext>,
        _dedup_group_id: Option<DedupGroupId>,
    ) -> Option<Self::ReporterId> {
        reporter
            .as_deref_mut()
            .and_then(|r| r.as_backend_source_build_reporter())
            .map(|r| r.on_queued(parent, self))
    }
    fn report_started(reporter: &mut Option<Box<dyn Reporter>>, id: Self::ReporterId) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_backend_source_build_reporter())
        {
            r.on_started(id, Box::new(futures::stream::empty()));
        }
    }
    fn report_finished(
        reporter: &mut Option<Box<dyn Reporter>>,
        id: Self::ReporterId,
        failed: bool,
    ) {
        if let Some(r) = reporter
            .as_deref_mut()
            .and_then(|r| r.as_backend_source_build_reporter())
        {
            r.on_finished(id, failed);
        }
    }
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
    InstantiateToolEnv(InstantiateToolEnvId),
    SourceBuild(SourceBuildId),
    BackendSourceBuild(BackendSourceBuildId),
}
