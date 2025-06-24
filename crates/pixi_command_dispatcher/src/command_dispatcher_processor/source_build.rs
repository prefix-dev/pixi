use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingSourceBuild, TaskResult};
use crate::{
    BuiltSource, CommandDispatcherError, CommandDispatcherErrorResultExt, Reporter,
    SourceBuildError,
    command_dispatcher::{CommandDispatcherContext, SourceBuildId, SourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`SourceBuildTask`] task was received.
    pub(crate) fn on_source_build(&mut self, task: SourceBuildTask) {
        // Notify the reporter that a new solve has been queued.
        let parent_context = task
            .parent
            .and_then(|context| self.reporter_context(context));
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_build_reporter)
            .map(|reporter| reporter.on_queued(parent_context, &task.spec));

        // Store information about the pending environment.
        let pending_id = self.source_builds.insert(PendingSourceBuild {
            tx: task.tx,
            reporter_id,
        });

        // Add to the list of pending tasks
        self.pending_source_builds
            .push_back((pending_id, task.spec));

        self.start_next_source_build();
    }

    fn start_next_source_build(&mut self) {
        let limit = self
            .inner
            .limits
            .max_concurrent_builds
            .unwrap_or(usize::MAX);
        while self.source_builds.len() - self.pending_source_builds.len() < limit {
            let Some((source_build_id, spec)) = self.pending_source_builds.pop_front() else {
                break;
            };

            let reporter_id = self.source_builds[source_build_id].reporter_id;

            // Open a channel to receive build output.
            let (tx, rx) = futures::channel::mpsc::unbounded();

            // Notify the reporter that the solve has started.
            if let Some((reporter, id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_source_build_reporter)
                .zip(reporter_id)
            {
                reporter.on_started(id, Box::new(rx));
            }

            // Add the task to the list of pending futures.
            let dispatcher = self.create_task_command_dispatcher(
                CommandDispatcherContext::SourceBuild(source_build_id),
            );
            self.pending_futures.push(
                spec.build(dispatcher, tx)
                    .map(move |result| TaskResult::SourceBuild(source_build_id, result))
                    .boxed_local(),
            );
        }
    }

    /// Called when a [`TaskResult::SolvePixiEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`crate::CommandDispatcher`] that issues it.
    pub(crate) fn on_source_build_result(
        &mut self,
        id: SourceBuildId,
        result: Result<BuiltSource, CommandDispatcherError<SourceBuildError>>,
    ) {
        let env = self
            .source_builds
            .remove(id)
            .expect("got a result for a source build that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_source_build_reporter)
            .zip(env.reporter_id)
        {
            reporter.on_finished(id)
        }

        // Notify the command dispatcher that the result is available.
        if let Some(result) = result.into_ok_or_failed() {
            // We can silently ignore the result if the task was cancelled.
            let _ = env.tx.send(result);
        };

        // Queue the next pending solve
        self.start_next_source_build();
    }
}
