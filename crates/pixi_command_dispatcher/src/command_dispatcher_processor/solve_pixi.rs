use futures::FutureExt;
use pixi_record::PixiRecord;

use super::{CommandDispatcherProcessor, PendingPixiEnvironment, TaskResult};
use crate::{
    CommandDispatcherError, CommandDispatcherErrorResultExt, Reporter, SolvePixiEnvironmentError,
    command_dispatcher::{
        CommandDispatcherContext, SolvePixiEnvironmentId, SolvePixiEnvironmentTask,
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::SolvePixiEnvironmentTask`]
    /// task was received.
    pub(crate) fn on_solve_pixi_environment(&mut self, task: SolvePixiEnvironmentTask) {
        // Notify the reporter that a new solve has been queued.
        let parent_context = task
            .context
            .and_then(|context| self.reporter_context(context));
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_solve_reporter)
            .map(|reporter| reporter.on_solve_queued(parent_context, &task.spec));

        // Store information about the pending environment.
        let pending_env_id = self.solve_pixi_environments.insert(PendingPixiEnvironment {
            tx: task.tx,
            reporter_id,
        });

        // Notify the reporter that the solve has started.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_solve_reporter)
            .zip(reporter_id)
        {
            reporter.on_solve_start(id)
        }

        // Add the task to the list of pending futures.
        let dispatcher = self.create_task_command_queue(
            CommandDispatcherContext::SolvePixiEnvironment(pending_env_id),
        );
        self.pending_futures.push(
            task.spec
                .solve(dispatcher)
                .map(move |result| TaskResult::SolvePixiEnvironment(pending_env_id, result))
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::SolvePixiEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`CommandDispatcher`] that issues it.
    pub(crate) fn on_solve_pixi_environment_result(
        &mut self,
        id: SolvePixiEnvironmentId,
        result: Result<Vec<PixiRecord>, CommandDispatcherError<SolvePixiEnvironmentError>>,
    ) {
        let env = self
            .solve_pixi_environments
            .remove(id)
            .expect("got a result for a conda environment that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_solve_reporter)
            .zip(env.reporter_id)
        {
            reporter.on_solve_finished(id)
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
