use futures::FutureExt;

use super::{CommandQueueProcessor, PendingInstallPixiEnvironment, TaskResult};
use crate::command_queue::{InstallPixiEnvironmentId, InstallPixiEnvironmentTask};
use crate::install_pixi::InstallPixiEnvironmentError;
use crate::{
    CommandQueueError, CommandQueueErrorResultExt, Reporter,
    command_queue::CommandQueueContext,
};

impl CommandQueueProcessor {
    /// Called when a [`super::ForegroundMessage::InstallPixiEnvironment`]
    /// task was received.
    pub(crate) fn on_install_pixi_environment(&mut self, task: InstallPixiEnvironmentTask) {
        // Notify the reporter that a new solve has been queued.
        let reporter_id = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_install_reporter)
            .map(|reporter| reporter.on_install_queued(&task.spec));

        // Store information about the pending environment.
        let pending_env_id = self
            .install_pixi_environment
            .insert(PendingInstallPixiEnvironment {
                tx: task.tx,
                reporter_id,
            });

        // Notify the reporter that the solve has started.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_pixi_install_reporter)
            .zip(reporter_id)
        {
            reporter.on_install_start(id)
        }

        // Add the task to the list of pending futures.
        let dispatcher = self
            .create_task_command_queue(CommandQueueContext::InstallPixiEnvironment(pending_env_id));
        self.pending_futures.push(
            task.spec
                .install(dispatcher)
                .map(move |result| TaskResult::InstallPixiEnvironment(pending_env_id, result))
                .boxed_local(),
        );
    }

    /// Called when a [`TaskResult::InstallPixiEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`CommandQueue`] that issues it.
    pub(crate) fn on_install_pixi_environment_result(
        &mut self,
        id: InstallPixiEnvironmentId,
        result: Result<(), CommandQueueError<InstallPixiEnvironmentError>>,
    ) {
        let env = self
            .install_pixi_environment
            .remove(id)
            .expect("got a result for a conda environment install that was not pending");

        // Notify the reporter that the solve finished.
        if let Some((reporter, id)) = self.reporter.as_deref_mut().and_then(Reporter::as_pixi_install_reporter).zip(env.reporter_id) {
            reporter.on_install_finished(id)
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
