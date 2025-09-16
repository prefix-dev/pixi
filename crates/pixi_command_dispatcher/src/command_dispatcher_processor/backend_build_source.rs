use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingBackendSourceBuild, TaskResult};
use crate::{
    BackendBuiltSource, CommandDispatcherError, CommandDispatcherErrorResultExt, Reporter,
    backend_source_build::BackendSourceBuildError,
    command_dispatcher::{BackendSourceBuildId, BackendSourceBuildTask},
};

impl CommandDispatcherProcessor {
    /// Called when a [`BackendBuildSourceTask`] task was received.
    pub(crate) fn on_backend_source_build(&mut self, task: BackendSourceBuildTask) {
        // Notify the reporter that a new solve has been queued.
        let parent_context = task
            .parent
            .and_then(|context| self.reporter_context(context));
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_backend_source_build_reporter)
            .map(|reporter| reporter.on_queued(parent_context, &task.spec));

        // Store information about the pending environment.
        let pending_id = self
            .backend_source_builds
            .insert(PendingBackendSourceBuild {
                tx: task.tx,
                reporter_id,
            });

        // Store the parent context for the task.
        if let Some(parent_context) = task.parent {
            self.parent_contexts
                .insert(pending_id.into(), parent_context);
        }

        // Add to the list of pending tasks
        self.pending_backend_source_builds.push_back((
            pending_id,
            task.spec,
            task.cancellation_token,
        ));

        self.start_next_backend_source_build();
    }

    fn start_next_backend_source_build(&mut self) {
        let limit = self
            .inner
            .limits
            .max_concurrent_builds
            .unwrap_or(usize::MAX);
        while self.backend_source_builds.len() - self.pending_backend_source_builds.len() < limit {
            let Some((backend_source_build_id, spec, cancellation_token)) =
                self.pending_backend_source_builds.pop_front()
            else {
                break;
            };

            let reporter_id = self.backend_source_builds[backend_source_build_id].reporter_id;

            // Open a channel to receive build output.
            let (tx, rx) = futures::channel::mpsc::unbounded();

            // Notify the reporter that the solve has started.
            if let Some((reporter, id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_backend_source_build_reporter)
                .zip(reporter_id)
            {
                reporter.on_started(id, Box::new(rx));
            }

            // Add the task to the list of pending futures.
            self.pending_futures.push(
                cancellation_token
                    .run_until_cancelled_owned(spec.build(tx))
                    .map(move |result| {
                        TaskResult::BackendSourceBuild(
                            backend_source_build_id,
                            result.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                        )
                    })
                    .boxed_local(),
            );
        }
    }

    /// Called when a [`TaskResult::BackendSourceBuild`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`crate::CommandDispatcher`] that issues it.
    pub(crate) fn on_backend_source_build_result(
        &mut self,
        id: BackendSourceBuildId,
        result: Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>>,
    ) {
        self.parent_contexts.remove(&id.into());
        let env = self
            .backend_source_builds
            .remove(id)
            .expect("got a result for a source build that was not pending");

        let result = result.into_ok_or_failed();

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_backend_source_build_reporter)
            .zip(env.reporter_id)
        {
            let failed = matches!(result, Some(Err(_)));
            reporter.on_finished(id, failed)
        }

        // Notify the command dispatcher that the result is available.
        if let Some(result) = result {
            // We can silently ignore the result if the task was cancelled.
            let _ = env.tx.send(result);
        };

        // Queue the next pending solve
        self.start_next_backend_source_build();
    }
}
