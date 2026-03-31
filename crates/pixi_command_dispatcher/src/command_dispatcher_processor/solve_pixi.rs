use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingPixiEnvironment, TaskResult};
use crate::{
    CommandDispatcherError, PixiEnvironmentSpec,
    command_dispatcher::{CommandDispatcherContext, SolvePixiEnvironmentTask},
    reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`crate::command_dispatcher::SolvePixiEnvironmentTask`]
    /// task was received.
    pub(crate) fn on_solve_pixi_environment(&mut self, task: SolvePixiEnvironmentTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let (reporter_id, cancellation_token) =
            self.start_slotmap_task(&task.spec, task.parent, task.cancellation_token);

        // Store information about the pending environment.
        let pending_env_id = self.solve_pixi_environments.insert(PendingPixiEnvironment {
            tx: task.tx,
            reporter_id,
        });

        if let Some(parent_context) = task.parent {
            self.parent_contexts
                .insert(pending_env_id.into(), parent_context);
        }

        // Notify the reporter that the solve has started.
        if let Some(id) = reporter_id {
            PixiEnvironmentSpec::report_started(&mut self.reporter, id);
        }

        let dispatcher_context = CommandDispatcherContext::SolvePixiEnvironment(pending_env_id);
        let reporter_context = self.reporter_context(dispatcher_context);
        let gateway_reporter = self
            .reporter
            .as_deref_mut()
            .and_then(|reporter| reporter.create_gateway_reporter(reporter_context));

        self.store_cancellation_token(dispatcher_context, cancellation_token.clone());

        // Add the task to the list of pending futures.
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.solve(dispatcher, gateway_reporter))
                .map(move |result| {
                    TaskResult::SolvePixiEnvironment(
                        pending_env_id,
                        Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }
}
