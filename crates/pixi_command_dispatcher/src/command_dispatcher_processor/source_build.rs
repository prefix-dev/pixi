use std::collections::hash_map::Entry;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter, SourceBuildError, SourceBuildResult,
    command_dispatcher::{CommandDispatcherContext, SourceBuildId, SourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceBuildTask`]
    /// task was received.
    pub(crate) fn on_source_build(&mut self, task: SourceBuildTask) {
        // Lookup the id of the source metadata to avoid deduplication.
        let source_build_id = {
            match self.source_build_ids.get(&task.spec) {
                Some(id) => *id,
                None => {
                    // If the source build is not in the map we need to create a new id for it.
                    let id = SourceBuildId(self.source_build_ids.len());
                    self.source_build_ids.insert(task.spec.clone(), id);
                    if let Some(parent) = task.parent {
                        self.parent_contexts.insert(id.into(), parent);
                    }
                    id
                }
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

                // Add the task to the list of pending futures.
                let dispatcher_context = CommandDispatcherContext::SourceBuild(source_build_id);
                let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
                self.pending_futures.push(
                    task.spec
                        .build(dispatcher)
                        .map(move |result| TaskResult::SourceBuild(source_build_id, result))
                        .boxed_local(),
                );
            }
        }
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
