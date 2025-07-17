use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingSourceBuild, TaskResult};
use crate::{
    BuiltSource, CommandDispatcherError, CommandDispatcherErrorResultExt, Reporter,
    SourceBuildError,
    command_dispatcher::{CommandDispatcherContext, SourceBuildId, SourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SourceBuildTask`]
    /// task was received.
    pub(crate) fn on_source_build(&mut self, task: SourceBuildTask) {
        // Notify the reporter that a new build has been queued.
        let parent_context = task
            .parent
            .and_then(|context| self.reporter_context(context));
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_build_reporter)
            .map(|reporter| reporter.on_queued(parent_context, &task.spec));

        // Store information about the pending environment.
        let pending_env_id = self.source_builds.insert(PendingSourceBuild {
            tx: task.tx,
            reporter_id,
        });

        if let Some(parent_context) = task.parent {
            self.parent_contexts
                .insert(pending_env_id.into(), parent_context);
        }

        // Notify the reporter that the build has started.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_build_reporter)
            .zip(reporter_id)
        {
            reporter.on_started(id)
        }

        let dispatcher_context = CommandDispatcherContext::SourceBuild(pending_env_id);

        // Add the task to the list of pending futures.
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
        self.pending_futures.push(
            task.spec
                .build(dispatcher)
                .map(move |result| TaskResult::SourceBuild(pending_env_id, result))
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
        result: Result<BuiltSource, CommandDispatcherError<SourceBuildError>>,
    ) {
        self.parent_contexts.remove(&id.into());
        let env = self
            .source_builds
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_build_reporter)
            .zip(env.reporter_id)
        {
            reporter.on_finished(id)
        }

        let Some(result) = result.into_ok_or_failed() else {
            // If the job was canceled, we can just drop the sending end
            // which will also cause a cancel on the receiving end.
            return;
        };

        // We can silently ignore the result if the task was cancelled.
        let _ = env.tx.send(result);
    }
}
