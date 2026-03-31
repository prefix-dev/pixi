use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingInstallPixiEnvironment, TaskResult};
use crate::{
    CommandDispatcherError,
    command_dispatcher::{CommandDispatcherContext, InstallPixiEnvironmentTask},
    install_pixi::InstallPixiEnvironmentSpec,
    reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::InstallPixiEnvironment`]
    /// task was received.
    pub(crate) fn on_install_pixi_environment(&mut self, task: InstallPixiEnvironmentTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        // Notify the reporter that a new solve has been queued.
        let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
        let reporter_id = task
            .spec
            .report_queued(&mut self.reporter, parent_context, None);

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
        if let Some(id) = reporter_id {
            InstallPixiEnvironmentSpec::report_started(&mut self.reporter, id);
        }

        // Create a reporter for the installation task.
        let dispatcher_context = CommandDispatcherContext::InstallPixiEnvironment(pending_env_id);
        let reporter_context = self.reporter_context(dispatcher_context);
        let install_reporter = self
            .reporter
            .as_mut()
            .and_then(|reporter| reporter.create_install_reporter(reporter_context));

        // Create a child cancellation token linked to parent's token (if any).
        let cancellation_token =
            self.get_child_cancellation_token(task.parent, task.cancellation_token);

        // Store the cancellation token for this context so child tasks can link to it.
        self.store_cancellation_token(dispatcher_context, cancellation_token.clone());

        // Add the task to the list of pending futures.
        let dispatcher = self.create_task_command_dispatcher(dispatcher_context);
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.install(dispatcher, install_reporter))
                .map(move |result| {
                    TaskResult::InstallPixiEnvironment(
                        pending_env_id,
                        Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }
}
