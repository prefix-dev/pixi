use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingSolveCondaEnvironment, TaskResult};
use crate::{
    CommandDispatcherError, SolveCondaEnvironmentSpec,
    command_dispatcher::SolveCondaEnvironmentTask, reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::SolveCondaEnvironment`] task
    /// was received.
    pub(crate) fn on_solve_conda_environment(&mut self, task: SolveCondaEnvironmentTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        // Notify the reporter that a new solve has been queued.
        let parent_context = task
            .parent
            .and_then(|context| self.reporter_context(context));
        let reporter_id = task
            .spec
            .report_queued(&mut self.reporter, parent_context, None);

        // Store information about the pending environment.
        let environment_id = self.conda_solves.insert(PendingSolveCondaEnvironment {
            tx: task.tx,
            reporter_id,
        });

        if let Some(parent) = task.parent {
            // Store the parent context for the task.
            self.parent_contexts.insert(environment_id.into(), parent);
        }

        // Create a child cancellation token linked to parent's token (if any).
        let cancellation_token =
            self.get_child_cancellation_token(task.parent, task.cancellation_token);

        // Add the environment to the list of pending environments.
        self.pending_conda_solves
            .push_back((environment_id, task.spec, cancellation_token));

        // Queue up as many solves as possible.
        self.start_next_conda_environment_solves();
    }

    /// Queue as many solves as possible within the allowed limits.
    pub(super) fn start_next_conda_environment_solves(&mut self) {
        use crate::command_dispatcher::CommandDispatcherContext;

        let limit = self
            .inner
            .limits
            .max_concurrent_solves
            .unwrap_or(usize::MAX);
        while self.conda_solves.len() - self.pending_conda_solves.len() < limit {
            let Some((environment_id, spec, cancellation_token)) =
                self.pending_conda_solves.pop_front()
            else {
                break;
            };

            let reporter_id = self.conda_solves[environment_id].reporter_id;

            // Notify the reporter that the solve has started.
            if let Some(id) = reporter_id {
                SolveCondaEnvironmentSpec::report_started(&mut self.reporter, id);
            }

            // Store the cancellation token for this context so child tasks can link to it.
            let context = CommandDispatcherContext::SolveCondaEnvironment(environment_id);
            self.store_cancellation_token(context, cancellation_token.clone());

            // Add the task to the list of pending futures.
            self.pending_futures.push(
                cancellation_token
                    .run_until_cancelled_owned(spec.solve())
                    .map(move |result| {
                        TaskResult::SolveCondaEnvironment(
                            environment_id,
                            Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                        )
                    })
                    .boxed_local(),
            );
        }
    }
}
