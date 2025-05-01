use futures::FutureExt;
use pixi_record::PixiRecord;

use super::{CommandQueueProcessor, PendingSolveCondaEnvironment, TaskResult};
use crate::{
    CommandQueueError, CondaSolveReporter,
    command_queue::{
        CommandQueueErrorResultExt, SolveCondaEnvironmentId, SolveCondaEnvironmentTask,
    },
};

impl CommandQueueProcessor {
    /// Called when a [`super::ForegroundMessage::SolveCondaEnvironment`] task
    /// was received.
    pub(crate) fn on_solve_conda_environment(&mut self, task: SolveCondaEnvironmentTask) {
        // Notify the reporter that a new solve has been queued.
        let reporter_id = self
            .reporter
            .as_mut()
            .map(|reporter| CondaSolveReporter::on_solve_queued(reporter.as_mut(), &task.env));

        // Store information about the pending environment.
        let environment_id = self.conda_solves.insert(PendingSolveCondaEnvironment {
            tx: task.tx,
            reporter_id,
        });

        // Add the environment to the list of pending environments.
        self.pending_conda_solves
            .push_back((environment_id, task.env));

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
            let Some((environment_id, spec)) = self.pending_conda_solves.pop_front() else {
                break;
            };

            let reporter_id = self.conda_solves[environment_id].reporter_id;

            // Notify the reporter that the solve has started.
            if let Some((reporter, id)) = self.reporter.as_mut().zip(reporter_id) {
                CondaSolveReporter::on_solve_start(reporter.as_mut(), id)
            }

            // Add the task to the list of pending futures.
            self.pending_futures.push(
                spec.solve()
                    .map(move |result| TaskResult::SolveCondaEnvironment(environment_id, result))
                    .boxed_local(),
            );
        }
    }

    /// Called when a [`TaskResult::SolveCondaEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`super::CommandQueue`] that issues it.
    pub(crate) fn on_solve_conda_environment_result(
        &mut self,
        id: SolveCondaEnvironmentId,
        result: Result<Vec<PixiRecord>, CommandQueueError<rattler_solve::SolveError>>,
    ) {
        let env = self
            .conda_solves
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self.reporter.as_mut().zip(env.reporter_id) {
            CondaSolveReporter::on_solve_finished(reporter.as_mut(), id)
        }

        // Notify the command queue that the result is available.
        if let Some(result) = result.into_ok_or_failed() {
            // We can silently ignore the result if the task was cancelled.
            let _ = env.tx.send(result);
        };

        // Queue the next pending solve
        self.start_next_conda_environment_solves();
    }
}
