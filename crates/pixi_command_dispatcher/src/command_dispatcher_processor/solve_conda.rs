use futures::FutureExt;
use pixi_record::PixiRecord;

use super::{CommandDispatcherProcessor, PendingSolveCondaEnvironment, TaskResult};
use crate::{
    CommandDispatcherError, CommandDispatcherErrorResultExt, Reporter,
    command_dispatcher::{SolveCondaEnvironmentId, SolveCondaEnvironmentTask},
    solve_conda::SolveCondaEnvironmentError,
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::SolveCondaEnvironment`] task
    /// was received.
    pub(crate) fn on_solve_conda_environment(&mut self, task: SolveCondaEnvironmentTask) {
        // Notify the reporter that a new solve has been queued.
        let parent_context = task
            .parent
            .and_then(|context| self.reporter_context(context));
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_conda_solve_reporter)
            .map(|reporter| reporter.on_queued(parent_context, &task.spec));

        // Store information about the pending environment.
        let environment_id = self.conda_solves.insert(PendingSolveCondaEnvironment {
            tx: task.tx,
            reporter_id,
        });

        if let Some(parent) = task.parent {
            // Store the parent context for the task.
            self.parent_contexts.insert(environment_id.into(), parent);
        }

        // Add the environment to the list of pending environments.
        self.pending_conda_solves
            .push_back((environment_id, task.spec, task.cancellation_token));

        // Queue up as many solves as possible.
        self.start_next_conda_environment_solves();
    }

    /// Queue as many solves as possible within the allowed limits.
    fn start_next_conda_environment_solves(&mut self) {
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
            if let Some((reporter, id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_conda_solve_reporter)
                .zip(reporter_id)
            {
                reporter.on_start(id)
            }

            // Add the task to the list of pending futures.
            self.pending_futures.push(
                cancellation_token
                    .run_until_cancelled_owned(spec.solve())
                    .map(move |result| {
                        TaskResult::SolveCondaEnvironment(
                            environment_id,
                            result.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                        )
                    })
                    .boxed_local(),
            );
        }
    }

    /// Called when a [`TaskResult::SolveCondaEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`super::CommandDispatcher`] that issues it.
    pub(crate) fn on_solve_conda_environment_result(
        &mut self,
        id: SolveCondaEnvironmentId,
        result: Result<Vec<PixiRecord>, CommandDispatcherError<SolveCondaEnvironmentError>>,
    ) {
        self.parent_contexts.remove(&id.into());
        let env = self
            .conda_solves
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_conda_solve_reporter)
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
        self.start_next_conda_environment_solves();
    }
}
