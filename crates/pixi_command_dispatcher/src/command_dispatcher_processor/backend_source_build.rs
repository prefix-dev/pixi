use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingBackendSourceBuild, TaskResult};
use crate::{CommandDispatcherError, Reporter, command_dispatcher::BackendSourceBuildTask};

impl CommandDispatcherProcessor {
    /// Called when a [`BackendBuildSourceTask`] task was received.
    pub(crate) fn on_backend_source_build(&mut self, task: BackendSourceBuildTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let (reporter_id, cancellation_token) =
            self.start_slotmap_task(&task.spec, task.parent, task.cancellation_token);

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
        self.pending_backend_source_builds
            .push_back((pending_id, *task.spec, cancellation_token));

        self.start_next_backend_source_build();
    }

    pub(super) fn start_next_backend_source_build(&mut self) {
        use crate::command_dispatcher::CommandDispatcherContext;

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

            // Store the cancellation token for this context so child tasks can link to it.
            let context = CommandDispatcherContext::BackendSourceBuild(backend_source_build_id);
            self.store_cancellation_token(context, cancellation_token.clone());

            // Add the task to the list of pending futures.
            self.pending_futures.push(
                cancellation_token
                    .run_until_cancelled_owned(spec.build(tx))
                    .map(move |result| {
                        TaskResult::BackendSourceBuild(
                            backend_source_build_id,
                            Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                        )
                    })
                    .boxed_local(),
            );
        }
    }
}
