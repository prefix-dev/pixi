use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingInstallPixiEnvironment, TaskResult};
use crate::{
    CommandDispatcherError, CommandDispatcherErrorResultExt, InstallPixiEnvironmentResult,
    Reporter,
    command_dispatcher::{
        CommandDispatcherContext, InstallPixiEnvironmentId, InstallPixiEnvironmentTask,
    },
    install_pixi::InstallPixiEnvironmentError,
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::InstallPixiEnvironment`]
    /// task was received.
    pub(crate) fn on_install_pixi_environment(&mut self, task: InstallPixiEnvironmentTask) {
        // Notify the reporter that a new solve has been queued.
        let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_install_reporter)
            .map(|reporter| reporter.on_queued(parent_context, &task.spec));

        // Store information about the pending environment.
        let pending_env_id = self
            .install_pixi_environment
            .insert(PendingInstallPixiEnvironment {
                tx: task.tx,
                reporter_id,
            });

        if let Some(parent_context) = task.parent {
            self.parent_contexts
                .insert(pending_env_id.into(), parent_context);
        }

        // Notify the reporter that the solve has started.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_install_reporter)
            .zip(reporter_id)
        {
            reporter.on_start(id)
        }

        // Create a reporter for the installation task.
        let dispatcher_context = CommandDispatcherContext::InstallPixiEnvironment(pending_env_id);
        let reporter_context = self.reporter_context(dispatcher_context);
        let install_reporter = self
            .reporter
            .as_mut()
            .and_then(|reporter| reporter.create_install_reporter(reporter_context));

        // Add the task to the list of pending futures.
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
        self.pending_futures.push(
            task.cancellation_token
                .run_until_cancelled_owned(task.spec.install(dispatcher, install_reporter))
                .map(move |result| {
                    TaskResult::InstallPixiEnvironment(
                        pending_env_id,
                        result.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                    )
                })
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::InstallPixiEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`crate::CommandDispatcher`] that issues it.
    pub(crate) fn on_install_pixi_environment_result(
        &mut self,
        id: InstallPixiEnvironmentId,
        result: Result<
            InstallPixiEnvironmentResult,
            CommandDispatcherError<InstallPixiEnvironmentError>,
        >,
    ) {
        self.parent_contexts.remove(&id.into());
        let env = self
            .install_pixi_environment
            .remove(id)
            .expect("got a result for a conda environment install that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_install_reporter)
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
