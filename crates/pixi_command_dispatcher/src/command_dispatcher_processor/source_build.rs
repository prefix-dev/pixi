use std::collections::hash_map::Entry;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter, SourceBuildError, SourceBuildResult, SourceBuildSpec,
    command_dispatcher::{CommandDispatcherContext, SourceBuildId, SourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Constructs a new [`SourceBuildId`] for the given `task`.
    fn gen_source_build_id(&mut self, task: &SourceBuildTask) -> SourceBuildId {
        let id = SourceBuildId(self.source_build_ids.len());
        self.source_build_ids.insert(task.spec.clone(), id);
        if let Some(parent) = task.parent {
            self.parent_contexts.insert(id.into(), parent);
        }
        id
    }

    /// Called when a [`crate::command_dispatcher::SourceBuildTask`]
    /// task was received.
    pub(crate) fn on_source_build(&mut self, task: SourceBuildTask) {
        // Lookup the id of the source metadata to avoid duplication.
        let source_build_id = {
            match self.source_build_ids.get(&task.spec) {
                Some(id) => *id,
                None => self.gen_source_build_id(&task),
            }
        };

        match self.source_build.entry(source_build_id) {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                PendingDeduplicatingTask::Pending(pending, _) => pending.push(task.tx),
                PendingDeduplicatingTask::Result(result, _) => {
                    let _ = task.tx.send(Ok(result.clone()));
                }
                PendingDeduplicatingTask::Errored => {
                    // Drop the sender, this will cause a cancellation on the other side.
                    drop(task.tx);
                }
            },
            Entry::Vacant(entry) => {
                entry.insert(PendingDeduplicatingTask::Pending(
                    vec![task.tx],
                    task.parent,
                ));

                // Notify the reporter that a new solve has been queued and started.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_build_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec));

                if let Some(reporter_id) = reporter_id {
                    self.source_build_reporters
                        .insert(source_build_id, reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_source_build_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }

                self.queue_source_build_task(source_build_id, task.spec, task.cancellation_token);
            }
        }
    }

    /// Queues a source build task to be executed.
    fn queue_source_build_task(
        &mut self,
        source_build_id: SourceBuildId,
        spec: SourceBuildSpec,
        cancellation_token: CancellationToken,
    ) {
        let dispatcher = self
            .create_task_command_dispatcher(CommandDispatcherContext::SourceBuild(source_build_id));

        let dispatcher_context = CommandDispatcherContext::SourceBuild(source_build_id);
        let reporter_context = self.reporter_context(dispatcher_context);
        let run_exports_reporter = self
            .reporter
            .as_mut()
            .and_then(|reporter| reporter.create_run_exports_reporter(reporter_context));

        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(spec.build(dispatcher, run_exports_reporter))
                .map(move |result| {
                    TaskResult::SourceBuild(
                        source_build_id,
                        result.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                    )
                })
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::SourceBuild`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`crate::CommandDispatcher`] that issues it.
    pub(crate) fn on_source_build_result(
        &mut self,
        id: SourceBuildId,
        result: Result<SourceBuildResult, CommandDispatcherError<SourceBuildError>>,
    ) {
        self.parent_contexts.remove(&id.into());
        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_build_reporter)
            .zip(self.source_build_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id);
        }

        self.source_build
            .get_mut(&id)
            .expect("cannot find pending task")
            .on_pending_result(result)
    }
}
