//! This module defines the [`CommandDispatcherProcessor`]. This is a task
//! spawned on the background that coordinates certain computation requests.
//!
//! Because the task runs in a single thread ownership of its resources is much
//! simpler since we easily acquire mutable access to its fields.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use crate::CommandDispatcherErrorResultExt;
use crate::command_dispatcher::TaskSpec;
use crate::source_build_cache_status::SourceBuildDeduplicationKey;
use crate::source_record::SourceRecordDeduplicationKey;
use crate::{
    BuildBackendMetadata, BuildBackendMetadataError, BuildBackendMetadataSpec, CommandDispatcher,
    CommandDispatcherError, DevSourceMetadata, DevSourceMetadataError, DevSourceMetadataSpec,
    InstallPixiEnvironmentResult, Reporter, ResolvedSourceRecord, SolveCondaEnvironmentSpec,
    SolvePixiEnvironmentError, SourceBuildCacheEntry, SourceBuildCacheStatusError,
    SourceBuildCacheStatusSpec, SourceBuildError, SourceBuildResult, SourceBuildSpec,
    SourceMetadata, SourceMetadataError, SourceMetadataSpec, SourceRecordError, SourceRecordSpec,
    backend_source_build::{BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildSpec},
    command_dispatcher::{
        BackendSourceBuildId, BuildBackendMetadataId, CommandDispatcherChannel,
        CommandDispatcherContext, CommandDispatcherData, DevSourceMetadataId, ForegroundMessage,
        GitCheckoutId, InstallPixiEnvironmentId, InstantiatedToolEnvId, SolveCondaEnvironmentId,
        SolvePixiEnvironmentId, SourceBuildCacheStatusId, SourceBuildId, SourceMetadataId,
        SourceRecordId, UrlCheckoutId,
        url::{UrlCheckout, UrlError},
    },
    executor::ExecutorFutures,
    install_pixi::InstallPixiEnvironmentError,
    instantiate_tool_env::InstantiateToolEnvironmentSpec,
    instantiate_tool_env::{InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult},
    reporter,
    solve_conda::SolveCondaEnvironmentError,
};
use futures::{StreamExt, future::LocalBoxFuture};
use itertools::Itertools;
use pixi_git::{GitError, resolver::RepositoryReference, source::Fetch};
use pixi_record::PixiRecord;
use pixi_spec::UrlSpec;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

mod backend_source_build;
mod build_backend_metadata;
pub(crate) mod dedup;
mod dev_source_metadata;
mod git;
mod install_pixi;
mod instantiate_tool_env;
mod solve_conda;
mod solve_pixi;
mod source_build;
mod source_build_cache_status;
mod source_metadata;
mod source_record;
mod url;

/// Runs the command_dispatcher background task
pub(crate) struct CommandDispatcherProcessor {
    /// The receiver for messages from a [`CommandDispatcher]`.
    receiver: mpsc::UnboundedReceiver<ForegroundMessage>,

    /// Keeps track of the parent context for each task that is being processed.
    parent_contexts: HashMap<CommandDispatcherContext, CommandDispatcherContext>,

    /// Keeps track of cancellation tokens for each task context.
    /// Used to create child tokens that are automatically cancelled when the parent is cancelled.
    cancellation_tokens: HashMap<CommandDispatcherContext, CancellationToken>,

    /// A weak reference to the sender. This is used to allow constructing new
    /// [`Dispatchers`] without keeping the channel alive if there are no
    /// dispatchers alive. This is important because the command_dispatcher
    /// background task is only stopped once all senders (and thus
    /// dispatchers) have been dropped.
    sender: mpsc::WeakUnboundedSender<ForegroundMessage>,

    /// Data associated with dispatchers.
    inner: Arc<CommandDispatcherData>,

    /// Conda environments that are currently being solved.
    conda_solves: slotmap::SlotMap<SolveCondaEnvironmentId, PendingSolveCondaEnvironment>,

    /// A list of conda environments that are pending to be solved. These have
    /// not yet been queued for processing.
    pending_conda_solves: VecDeque<(
        SolveCondaEnvironmentId,
        SolveCondaEnvironmentSpec,
        CancellationToken,
    )>,

    /// Pixi environments that are currently being solved.
    solve_pixi_environments: slotmap::SlotMap<SolvePixiEnvironmentId, PendingPixiEnvironment>,

    /// Pixi environments that are currently being solved.
    install_pixi_environment:
        slotmap::SlotMap<InstallPixiEnvironmentId, PendingInstallPixiEnvironment>,

    /// Build backend metadata tasks that are currently being processed.
    build_backend_metadata: dedup::DedupTaskRegistry<
        BuildBackendMetadataSpec,
        BuildBackendMetadataId,
        Arc<BuildBackendMetadata>,
        BuildBackendMetadataError,
    >,
    build_backend_metadata_reporters:
        HashMap<BuildBackendMetadataId, Vec<reporter::BuildBackendMetadataId>>,

    /// Source metadata tasks (not deduplicated; fans out to deduplicated
    /// SourceRecord tasks). Uses a unique counter as key so every request
    /// creates a new task.
    source_metadata:
        dedup::DedupTaskRegistry<usize, SourceMetadataId, Arc<SourceMetadata>, SourceMetadataError>,
    source_metadata_reporters: HashMap<SourceMetadataId, Vec<reporter::SourceMetadataId>>,
    source_metadata_id_counter: usize,

    /// Source record requests (per name+variant) that are currently being processed.
    source_record: dedup::DedupTaskRegistry<
        SourceRecordDeduplicationKey,
        SourceRecordId,
        Arc<ResolvedSourceRecord>,
        SourceRecordError,
    >,
    source_record_reporters: HashMap<SourceRecordId, Vec<reporter::SourceRecordId>>,

    /// A mapping of instantiated tool environments
    instantiated_tool_envs: dedup::DedupTaskRegistry<
        String,
        InstantiatedToolEnvId,
        InstantiateToolEnvironmentResult,
        InstantiateToolEnvironmentError,
    >,
    instantiated_tool_envs_reporters:
        HashMap<InstantiatedToolEnvId, Vec<reporter::InstantiateToolEnvId>>,

    /// Git checkouts in the process of being checked out, or already
    /// checked out.
    git_checkouts: dedup::DedupTaskRegistry<RepositoryReference, GitCheckoutId, Fetch, GitError>,
    git_checkout_reporters: HashMap<GitCheckoutId, Vec<reporter::GitCheckoutId>>,

    /// Url checkouts in the process of being checked out, or already
    /// checked out.
    url_checkouts: dedup::DedupTaskRegistry<UrlSpec, UrlCheckoutId, UrlCheckout, UrlError>,
    url_checkout_reporters: HashMap<UrlCheckoutId, Vec<reporter::UrlCheckoutId>>,

    /// Source builds that are currently being processed.
    source_build: dedup::DedupTaskRegistry<
        SourceBuildSpec,
        SourceBuildId,
        SourceBuildResult,
        SourceBuildError,
    >,
    source_build_reporters: HashMap<SourceBuildId, Vec<reporter::SourceBuildId>>,

    /// Queries of source builds cache that are currently being processed.
    source_build_cache_status: dedup::DedupTaskRegistry<
        SourceBuildDeduplicationKey,
        SourceBuildCacheStatusId,
        Arc<SourceBuildCacheEntry>,
        SourceBuildCacheStatusError,
    >,

    /// Dev source metadata requests that are currently being processed.
    dev_source_metadata: dedup::DedupTaskRegistry<
        DevSourceMetadataSpec,
        DevSourceMetadataId,
        DevSourceMetadata,
        DevSourceMetadataError,
    >,

    /// Backend source builds that are currently being processed.
    backend_source_builds: slotmap::SlotMap<BackendSourceBuildId, PendingBackendSourceBuild>,
    pending_backend_source_builds: VecDeque<(
        BackendSourceBuildId,
        BackendSourceBuildSpec,
        CancellationToken,
    )>,

    /// Keeps track of all pending futures. We poll them manually instead of
    /// spawning them so they can be `!Send` and because they are dropped when
    /// this instance is dropped.
    pending_futures: ExecutorFutures<LocalBoxFuture<'static, TaskResult>>,

    /// Monitoring futures for subscriber cancellation of deduplicated tasks.
    /// Kept separate from `pending_futures` so they never block task futures
    /// in Serial executor mode.
    monitor_futures: ExecutorFutures<LocalBoxFuture<'static, CommandDispatcherContext>>,

    /// The reporter to use for reporting progress
    reporter: Option<Box<dyn Reporter>>,
}

type BoxedDispatcherResult<T, E> = Box<Result<T, CommandDispatcherError<E>>>;

/// A result of a task that was executed by the command_dispatcher background
/// task.
enum TaskResult {
    SolveCondaEnvironment(
        SolveCondaEnvironmentId,
        BoxedDispatcherResult<Vec<PixiRecord>, SolveCondaEnvironmentError>,
    ),
    SolvePixiEnvironment(
        SolvePixiEnvironmentId,
        BoxedDispatcherResult<Vec<PixiRecord>, SolvePixiEnvironmentError>,
    ),
    BuildBackendMetadata(
        BuildBackendMetadataId,
        BoxedDispatcherResult<Arc<BuildBackendMetadata>, BuildBackendMetadataError>,
    ),
    SourceMetadata(
        SourceMetadataId,
        BoxedDispatcherResult<Arc<SourceMetadata>, SourceMetadataError>,
    ),
    SourceRecord(
        SourceRecordId,
        BoxedDispatcherResult<Arc<ResolvedSourceRecord>, SourceRecordError>,
    ),
    GitCheckedOut(GitCheckoutId, BoxedDispatcherResult<Fetch, GitError>),
    UrlCheckedOut(UrlCheckoutId, BoxedDispatcherResult<UrlCheckout, UrlError>),
    InstallPixiEnvironment(
        InstallPixiEnvironmentId,
        BoxedDispatcherResult<InstallPixiEnvironmentResult, InstallPixiEnvironmentError>,
    ),
    InstantiateToolEnv(
        InstantiatedToolEnvId,
        BoxedDispatcherResult<InstantiateToolEnvironmentResult, InstantiateToolEnvironmentError>,
    ),
    SourceBuild(
        SourceBuildId,
        BoxedDispatcherResult<SourceBuildResult, SourceBuildError>,
    ),
    QuerySourceBuildCache(
        SourceBuildCacheStatusId,
        BoxedDispatcherResult<Arc<SourceBuildCacheEntry>, SourceBuildCacheStatusError>,
    ),
    DevSourceMetadata(
        DevSourceMetadataId,
        BoxedDispatcherResult<DevSourceMetadata, DevSourceMetadataError>,
    ),
    BackendSourceBuild(
        BackendSourceBuildId,
        BoxedDispatcherResult<BackendBuiltSource, BackendSourceBuildError>,
    ),
}

/// Maps a slot-map based task spec to its pending entry on
/// [`CommandDispatcherProcessor`].
///
/// Each implementation knows which slotmap field holds its pending entries
/// and how to extract the `oneshot::Sender` and reporter ID.
trait HasSlotmapTaskFields: TaskSpec + reporter::Reportable {
    type Id: slotmap::Key;

    /// Removes the pending entry from the processor's slotmap and returns
    /// the sender and reporter ID.
    fn remove_pending(
        proc: &mut CommandDispatcherProcessor,
        id: Self::Id,
    ) -> (
        oneshot::Sender<Result<Self::Output, Self::Error>>,
        Option<Self::ReporterId>,
    );

    /// Constructs the context variant for this task's ID.
    fn context(id: Self::Id) -> CommandDispatcherContext;
}

impl HasSlotmapTaskFields for SolveCondaEnvironmentSpec {
    type Id = SolveCondaEnvironmentId;
    fn remove_pending(
        proc: &mut CommandDispatcherProcessor,
        id: Self::Id,
    ) -> (
        oneshot::Sender<Result<Self::Output, Self::Error>>,
        Option<Self::ReporterId>,
    ) {
        let entry = proc
            .conda_solves
            .remove(id)
            .expect("got a result for a conda environment that was not pending");
        (entry.tx, entry.reporter_id)
    }
    fn context(id: Self::Id) -> CommandDispatcherContext {
        CommandDispatcherContext::SolveCondaEnvironment(id)
    }
}

impl HasSlotmapTaskFields for crate::PixiEnvironmentSpec {
    type Id = SolvePixiEnvironmentId;
    fn remove_pending(
        proc: &mut CommandDispatcherProcessor,
        id: Self::Id,
    ) -> (
        oneshot::Sender<Result<Self::Output, Self::Error>>,
        Option<Self::ReporterId>,
    ) {
        let entry = proc
            .solve_pixi_environments
            .remove(id)
            .expect("got a result for a pixi environment that was not pending");
        (entry.tx, entry.reporter_id)
    }
    fn context(id: Self::Id) -> CommandDispatcherContext {
        CommandDispatcherContext::SolvePixiEnvironment(id)
    }
}

impl HasSlotmapTaskFields for crate::install_pixi::InstallPixiEnvironmentSpec {
    type Id = InstallPixiEnvironmentId;
    fn remove_pending(
        proc: &mut CommandDispatcherProcessor,
        id: Self::Id,
    ) -> (
        oneshot::Sender<Result<Self::Output, Self::Error>>,
        Option<Self::ReporterId>,
    ) {
        let entry = proc
            .install_pixi_environment
            .remove(id)
            .expect("got a result for a pixi environment install that was not pending");
        (entry.tx, entry.reporter_id)
    }
    fn context(id: Self::Id) -> CommandDispatcherContext {
        CommandDispatcherContext::InstallPixiEnvironment(id)
    }
}

impl HasSlotmapTaskFields for Box<BackendSourceBuildSpec> {
    type Id = BackendSourceBuildId;
    fn remove_pending(
        proc: &mut CommandDispatcherProcessor,
        id: Self::Id,
    ) -> (
        oneshot::Sender<Result<Self::Output, Self::Error>>,
        Option<Self::ReporterId>,
    ) {
        let entry = proc
            .backend_source_builds
            .remove(id)
            .expect("got a result for a source build that was not pending");
        (entry.tx, entry.reporter_id)
    }
    fn context(id: Self::Id) -> CommandDispatcherContext {
        CommandDispatcherContext::BackendSourceBuild(id)
    }
}

/// Information about a pending conda environment solve.
struct PendingSolveCondaEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolveCondaEnvironmentError>>,
    reporter_id: Option<reporter::CondaSolveId>,
}

struct PendingBackendSourceBuild {
    tx: oneshot::Sender<Result<BackendBuiltSource, BackendSourceBuildError>>,
    reporter_id: Option<reporter::BackendSourceBuildId>,
}

impl PendingTask<Box<BackendSourceBuildSpec>> for PendingBackendSourceBuild {
    fn into_parts(
        self,
    ) -> (
        oneshot::Sender<Result<BackendBuiltSource, BackendSourceBuildError>>,
        Option<reporter::BackendSourceBuildId>,
    ) {
        (self.tx, self.reporter_id)
    }
}

/// Information about a pending pixi environment solve. This is used by the
/// background task to keep track of which command_dispatcher is awaiting the
/// result.
struct PendingPixiEnvironment {
    tx: oneshot::Sender<Result<Vec<PixiRecord>, SolvePixiEnvironmentError>>,
    reporter_id: Option<reporter::PixiSolveId>,
}

struct PendingInstallPixiEnvironment {
    tx: oneshot::Sender<Result<InstallPixiEnvironmentResult, InstallPixiEnvironmentError>>,
    reporter_id: Option<reporter::PixiInstallId>,
}

impl CommandDispatcherProcessor {
    /// Spawns a new background task that will handle the orchestration of all
    /// the dispatchers.
    ///
    /// The task is spawned on its own dedicated thread but uses the tokio
    /// runtime of the current thread to facilitate concurrency. Futures that
    /// are spawned on the background thread can still fully utilize tokio's
    /// runtime. Similarly, futures using tokios IO eventloop still work as
    /// expected.
    pub fn spawn(
        inner: Arc<CommandDispatcherData>,
        reporter: Option<Box<dyn Reporter>>,
    ) -> (
        mpsc::UnboundedSender<ForegroundMessage>,
        std::thread::JoinHandle<()>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let weak_tx = tx.downgrade();
        let rt = tokio::runtime::Handle::current();
        let join_handle = std::thread::spawn(move || {
            let task = Self {
                receiver: rx,
                parent_contexts: HashMap::new(),
                cancellation_tokens: HashMap::new(),
                sender: weak_tx,
                conda_solves: slotmap::SlotMap::default(),
                pending_conda_solves: VecDeque::new(),
                solve_pixi_environments: slotmap::SlotMap::default(),
                install_pixi_environment: slotmap::SlotMap::default(),
                build_backend_metadata: Default::default(),
                build_backend_metadata_reporters: HashMap::default(),
                source_metadata: Default::default(),
                source_metadata_reporters: HashMap::default(),
                source_metadata_id_counter: 0,
                source_record: Default::default(),
                source_record_reporters: HashMap::default(),
                instantiated_tool_envs: Default::default(),
                instantiated_tool_envs_reporters: HashMap::default(),
                git_checkouts: Default::default(),
                git_checkout_reporters: HashMap::default(),
                url_checkouts: Default::default(),
                url_checkout_reporters: HashMap::default(),
                source_build: Default::default(),
                source_build_reporters: HashMap::default(),
                source_build_cache_status: Default::default(),
                dev_source_metadata: Default::default(),
                backend_source_builds: Default::default(),
                pending_backend_source_builds: Default::default(),
                pending_futures: ExecutorFutures::new(inner.executor),
                monitor_futures: ExecutorFutures::new(inner.executor),
                inner,
                reporter,
            };
            rt.block_on(task.run());
        });
        (tx, join_handle)
    }

    /// The main loop of the command_dispatcher background task. This function
    /// will run until all dispatchers have been dropped.
    async fn run(mut self) {
        tracing::trace!("Dispatch background task has started");
        if let Some(reporter) = self.reporter.as_mut() {
            reporter.on_start();
        }
        loop {
            tokio::select! {
                message = self.receiver.recv() => {
                    match message {
                        Some(message) => self.on_message(message),
                        None => {
                            // If all the senders are dropped, the receiver will be closed. When this
                            // happens, we can stop the command_dispatcher. All remaining tasks will be dropped
                            // as `self.pending_futures` is dropped.
                            break;
                        }
                    }
                }
                Some(result) = self.pending_futures.next() => {
                    self.on_result(result);
                }
                Some(context) = self.monitor_futures.next() => {
                    self.on_subscriber_cancelled(context);
                }
            }
        }
        if let Some(reporter) = self.reporter.as_mut() {
            reporter.on_finished();
        }
        tracing::trace!("Dispatch background task has finished");
    }

    /// Called when a message was received from either the command_dispatcher or
    /// another task.
    fn on_message(&mut self, message: ForegroundMessage) {
        match message {
            ForegroundMessage::SolveCondaEnvironment(task) => self.on_solve_conda_environment(task),
            ForegroundMessage::SolvePixiEnvironment(task) => self.on_solve_pixi_environment(task),
            ForegroundMessage::InstallPixiEnvironment(task) => {
                self.on_install_pixi_environment(task)
            }
            ForegroundMessage::InstantiateToolEnvironment(task) => {
                self.on_instantiate_tool_environment(task)
            }
            ForegroundMessage::BuildBackendMetadata(task) => self.on_build_backend_metadata(task),
            ForegroundMessage::GitCheckout(task) => self.on_checkout_git(task),
            ForegroundMessage::UrlCheckout(task) => self.on_checkout_url(task),
            ForegroundMessage::SourceBuild(task) => self.on_source_build(task),
            ForegroundMessage::QuerySourceBuildCache(task) => {
                self.on_source_build_cache_status(task)
            }
            ForegroundMessage::DevSourceMetadata(task) => self.on_dev_source_metadata(task),
            ForegroundMessage::ClearReporter(sender) => self.clear_reporter(sender),
            ForegroundMessage::ClearFilesystemCaches(sender) => {
                self.clear_filesystem_caches(sender)
            }
            ForegroundMessage::SourceMetadata(task) => self.on_source_metadata(task),
            ForegroundMessage::SourceRecord(task) => self.on_source_record(task),
            ForegroundMessage::BackendSourceBuild(task) => self.on_backend_source_build(task),
        }
    }

    /// Called when the result of a task was received.
    fn on_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::SolveCondaEnvironment(id, result) => {
                self.complete_slotmap_task::<SolveCondaEnvironmentSpec>(id, *result);
                self.start_next_conda_environment_solves();
            }
            TaskResult::SolvePixiEnvironment(id, result) => {
                self.complete_slotmap_task::<crate::PixiEnvironmentSpec>(id, *result);
            }
            TaskResult::InstallPixiEnvironment(id, result) => {
                self.complete_slotmap_task::<crate::install_pixi::InstallPixiEnvironmentSpec>(
                    id, *result,
                );
            }
            TaskResult::BuildBackendMetadata(id, result) => {
                self.complete_dedup_task::<BuildBackendMetadataSpec>(
                    CommandDispatcherContext::BuildBackendMetadata(id),
                    id,
                    *result,
                );
            }
            TaskResult::GitCheckedOut(id, result) => {
                self.complete_dedup_task::<pixi_git::GitUrl>(
                    CommandDispatcherContext::GitCheckout(id),
                    id,
                    *result,
                );
            }
            TaskResult::UrlCheckedOut(id, result) => {
                self.complete_dedup_task::<pixi_spec::UrlSpec>(
                    CommandDispatcherContext::UrlCheckout(id),
                    id,
                    *result,
                );
            }
            TaskResult::InstantiateToolEnv(id, result) => {
                self.complete_dedup_task::<InstantiateToolEnvironmentSpec>(
                    CommandDispatcherContext::InstantiateToolEnv(id),
                    id,
                    *result,
                );
            }
            TaskResult::SourceBuild(id, result) => {
                self.complete_dedup_task::<SourceBuildSpec>(
                    CommandDispatcherContext::SourceBuild(id),
                    id,
                    *result,
                );
            }
            TaskResult::SourceMetadata(id, result) => {
                self.complete_dedup_task::<SourceMetadataSpec>(
                    CommandDispatcherContext::SourceMetadata(id),
                    id,
                    *result,
                );
            }
            TaskResult::SourceRecord(id, result) => {
                self.complete_dedup_task::<SourceRecordSpec>(
                    CommandDispatcherContext::SourceRecord(id),
                    id,
                    *result,
                );
            }
            TaskResult::BackendSourceBuild(id, result) => {
                self.complete_slotmap_task::<Box<BackendSourceBuildSpec>>(id, *result);
                self.start_next_backend_source_build();
            }
            TaskResult::QuerySourceBuildCache(id, result) => {
                self.complete_dedup_task::<SourceBuildCacheStatusSpec>(
                    CommandDispatcherContext::QuerySourceBuildCache(id),
                    id,
                    *result,
                );
            }
            TaskResult::DevSourceMetadata(id, result) => {
                self.complete_dedup_task::<DevSourceMetadataSpec>(
                    CommandDispatcherContext::DevSourceMetadata(id),
                    id,
                    *result,
                );
            }
        }
    }

    /// Constructs a new [`CommandDispatcher`] that can be used for tasks
    /// constructed by the command dispatcher process itself.
    fn create_task_command_dispatcher(
        &self,
        context: CommandDispatcherContext,
    ) -> CommandDispatcher {
        CommandDispatcher {
            channel: Some(CommandDispatcherChannel::Weak(self.sender.clone())),
            context: Some(context),
            data: self.inner.clone(),
            processor_handle: None,
        }
    }

    fn reporter_context(
        &self,
        context: CommandDispatcherContext,
    ) -> Option<reporter::ReporterContext> {
        let mut parent_context = Some(context);
        while let Some(context) = parent_context.take() {
            parent_context = match context {
                CommandDispatcherContext::SolveCondaEnvironment(id) => {
                    return self.conda_solves[id]
                        .reporter_id
                        .map(reporter::ReporterContext::SolveConda);
                }
                CommandDispatcherContext::SolvePixiEnvironment(id) => {
                    return self.solve_pixi_environments[id]
                        .reporter_id
                        .map(reporter::ReporterContext::SolvePixi);
                }
                CommandDispatcherContext::BuildBackendMetadata(id) => {
                    if let Some(context) = self
                        .build_backend_metadata_reporters
                        .get(&id)
                        .and_then(|ids| ids.first().copied())
                        .map(reporter::ReporterContext::BuildBackendMetadata)
                    {
                        return Some(context);
                    }

                    self.parent_contexts
                        .get(&CommandDispatcherContext::BuildBackendMetadata(id))
                        .copied()
                }
                CommandDispatcherContext::SourceMetadata(id) => {
                    if let Some(context) = self
                        .source_metadata_reporters
                        .get(&id)
                        .and_then(|ids| ids.first().copied())
                        .map(reporter::ReporterContext::SourceMetadata)
                    {
                        return Some(context);
                    }

                    self.parent_contexts
                        .get(&CommandDispatcherContext::SourceMetadata(id))
                        .copied()
                }
                CommandDispatcherContext::SourceRecord(id) => {
                    if let Some(context) = self
                        .source_record_reporters
                        .get(&id)
                        .and_then(|ids| ids.first().copied())
                        .map(reporter::ReporterContext::SourceRecord)
                    {
                        return Some(context);
                    }

                    self.parent_contexts
                        .get(&CommandDispatcherContext::SourceRecord(id))
                        .copied()
                }
                CommandDispatcherContext::InstallPixiEnvironment(id) => {
                    return self.install_pixi_environment[id]
                        .reporter_id
                        .map(reporter::ReporterContext::InstallPixi);
                }
                CommandDispatcherContext::InstantiateToolEnv(id) => {
                    if let Some(context) = self
                        .instantiated_tool_envs_reporters
                        .get(&id)
                        .and_then(|ids| ids.first().copied())
                        .map(reporter::ReporterContext::InstantiateToolEnv)
                    {
                        return Some(context);
                    }

                    self.parent_contexts
                        .get(&CommandDispatcherContext::InstantiateToolEnv(id))
                        .copied()
                }
                CommandDispatcherContext::SourceBuild(id) => {
                    if let Some(context) = self
                        .source_build_reporters
                        .get(&id)
                        .and_then(|ids| ids.first().copied())
                        .map(reporter::ReporterContext::SourceBuild)
                    {
                        return Some(context);
                    }

                    self.parent_contexts
                        .get(&CommandDispatcherContext::SourceBuild(id))
                        .copied()
                }
                CommandDispatcherContext::BackendSourceBuild(id) => {
                    return self.backend_source_builds[id]
                        .reporter_id
                        .map(reporter::ReporterContext::BackendSourceBuild);
                }
                CommandDispatcherContext::QuerySourceBuildCache(_id) => {
                    return None;
                }
                CommandDispatcherContext::DevSourceMetadata(_id) => {
                    return None;
                }
                CommandDispatcherContext::GitCheckout(_id) => {
                    return None;
                }
                CommandDispatcherContext::UrlCheckout(_id) => {
                    return None;
                }
            };
        }

        None
    }

    /// Called to clear the reporter.
    fn clear_reporter(&mut self, sender: oneshot::Sender<()>) {
        if let Some(reporter) = self.reporter.as_mut() {
            reporter.on_clear()
        }
        let _ = sender.send(());
    }

    /// Clears cached results based on the filesystem, preserving in-flight tasks.
    fn clear_filesystem_caches(&mut self, sender: oneshot::Sender<()>) {
        self.inner.glob_hash_cache.clear();

        // Clear source build cache status, preserving in-flight tasks.
        self.source_build_cache_status.clear_completed();

        let _ = sender.send(());
    }

    /// Returns true if by following the parent chain of the `parent` context we
    /// stumble on `id`.
    pub fn contains_cycle<T: TryFrom<CommandDispatcherContext> + PartialEq>(
        &self,
        id: T,
        parent: Option<CommandDispatcherContext>,
    ) -> bool {
        std::iter::successors(parent, |ctx| self.parent_contexts.get(ctx).cloned())
            .filter_map(|context| T::try_from(context).ok())
            .contains(&id)
    }

    /// Creates a child cancellation token linked to the parent's token.
    ///
    /// If the parent context has a stored cancellation token, creates a child token
    /// that will be automatically cancelled when the parent is cancelled.
    /// Otherwise, returns the provided token unchanged.
    fn get_child_cancellation_token(
        &self,
        parent: Option<CommandDispatcherContext>,
        token: CancellationToken,
    ) -> CancellationToken {
        parent
            .and_then(|ctx| self.cancellation_tokens.get(&ctx))
            .map(|parent_token| parent_token.child_token())
            .unwrap_or(token)
    }

    /// Stores a cancellation token for the given context.
    fn store_cancellation_token(
        &mut self,
        context: CommandDispatcherContext,
        token: CancellationToken,
    ) {
        self.cancellation_tokens.insert(context, token);
    }

    /// Handles the result of a slot-map based (non-deduplicated) task:
    /// removes the pending entry, parent context, cancellation token,
    /// notifies the reporter, and sends the result to the waiting caller.
    fn complete_slotmap_task<S: HasSlotmapTaskFields>(
        &mut self,
        id: S::Id,
        result: Result<S::Output, CommandDispatcherError<S::Error>>,
    ) {
        let context = S::context(id);
        let (tx, reporter_id) = S::remove_pending(self, id);
        self.parent_contexts.remove(&context);
        self.complete_task_token(context, &result);
        if let Some(reporter_id) = reporter_id {
            S::report_finished(&mut self.reporter, reporter_id, result.is_err());
        }
        if let Some(result) = result.into_ok_or_failed() {
            let _ = tx.send(result);
        }
    }

    /// Handles the cancellation token for a completed task based on its result.
    ///
    /// - On **cancellation**: removes and cancels the token, propagating
    ///   cancellation to any child tasks.
    /// - On **success or error**: removes the token without cancelling it,
    ///   allowing child tasks to continue running and report their own results.
    fn complete_task_token<T, E>(
        &mut self,
        context: CommandDispatcherContext,
        result: &Result<T, CommandDispatcherError<E>>,
    ) {
        match result {
            Err(CommandDispatcherError::Cancelled) => {
                // Truly cancelled -- propagate to children
                if let Some(token) = self.cancellation_tokens.remove(&context) {
                    token.cancel();
                }
            }
            _ => {
                // Success or error -- remove token, do NOT cancel children
                self.cancellation_tokens.remove(&context);
            }
        }
    }

    /// Returns true if the parent context has been explicitly cancelled.
    ///
    /// A missing token means the parent completed (success or error) and
    /// new child tasks should still be allowed to proceed. Only an
    /// explicitly cancelled token blocks new children.
    ///
    /// Handles the result of a deduplicated task: removes the parent context,
    /// forwards the result to the dedup registry, and notifies reporters.
    fn complete_dedup_task<S: HasDedupTaskFields>(
        &mut self,
        context: CommandDispatcherContext,
        id: S::Id,
        result: Result<S::Ok, CommandDispatcherError<S::Err>>,
    ) {
        let fields = S::dedup_task_fields(self);
        fields.parent_contexts.remove(&context);
        let failed = result.is_err();
        let reporter_ids = fields.reporter_map.remove(&id);
        fields.dedup.on_result(id, result);
        if let Some(reporter_ids) = reporter_ids {
            for reporter_id in reporter_ids {
                S::report_finished(fields.reporter, reporter_id, failed);
            }
        }
    }

    fn is_parent_cancelled(&self, parent: Option<CommandDispatcherContext>) -> bool {
        parent.is_some_and(|ctx| {
            self.cancellation_tokens
                .get(&ctx)
                .is_some_and(|token| token.is_cancelled())
        })
    }

    /// Dispatches a subscriber cancellation event to the appropriate
    /// dedup task registry.
    fn on_subscriber_cancelled(&mut self, context: CommandDispatcherContext) {
        match context {
            CommandDispatcherContext::QuerySourceBuildCache(id) => {
                self.source_build_cache_status.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::DevSourceMetadata(id) => {
                self.dev_source_metadata.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::InstantiateToolEnv(id) => {
                self.instantiated_tool_envs.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::SourceRecord(id) => {
                self.source_record.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::SourceMetadata(id) => {
                self.source_metadata.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::BuildBackendMetadata(id) => {
                self.build_backend_metadata.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::SourceBuild(id) => {
                self.source_build.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::GitCheckout(id) => {
                self.git_checkouts.on_subscriber_cancelled(id);
            }
            CommandDispatcherContext::UrlCheckout(id) => {
                self.url_checkouts.on_subscriber_cancelled(id);
            }
            // Non-dedup tasks never push subscriber monitors.
            _ => unreachable!("subscriber cancellation for non-dedup context: {context:?}"),
        }
    }

    /// Pushes a monitoring future that fires when the given caller token is
    /// cancelled (i.e. the caller dropped their future). The resulting
    /// event on `monitor_futures` triggers the registry to check whether
    /// all subscribers are gone.
    fn push_subscriber_monitor(
        &mut self,
        context: CommandDispatcherContext,
        caller_token: CancellationToken,
    ) {
        use futures::FutureExt;

        self.monitor_futures.push(
            async move {
                caller_token.cancelled().await;
                context
            }
            .boxed_local(),
        );
    }
}
