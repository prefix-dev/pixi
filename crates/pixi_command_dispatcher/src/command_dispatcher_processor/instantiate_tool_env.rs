use futures::FutureExt;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError,
    command_dispatcher::{CommandDispatcherContext, InstantiatedToolEnvId, Task},
    instantiate_tool_env::InstantiateToolEnvironmentSpec,
    reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::InstantiateToolEnvironment`]
    /// task was received.
    pub(crate) fn on_instantiate_tool_environment(
        &mut self,
        task: Task<InstantiateToolEnvironmentSpec>,
    ) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let action = self.instantiated_tool_envs.on_task(
            task.spec.cache_key(),
            task.tx,
            InstantiatedToolEnvId,
        );
        let parent_reporter_context = task.parent.and_then(|ctx| self.reporter_context(ctx));

        let Some(NewDedupTask {
            id,
            cancellation_token,
            context,
        }) = Self::start_dedup_task(
            self,
            action,
            &task.spec,
            task.parent,
            task.cancellation_token,
            parent_reporter_context,
            CommandDispatcherContext::InstantiateToolEnv,
        )
        else {
            return;
        };

        if let Some(reporter_id) = self
            .instantiated_tool_envs_reporters
            .get(&id)
            .and_then(|ids| ids.last().copied())
        {
            InstantiateToolEnvironmentSpec::report_started(&mut self.reporter, reporter_id);
        }

        let command_queue = self.create_task_command_dispatcher(context);
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(task.spec.instantiate(command_queue))
                .map(move |result| {
                    TaskResult::InstantiateToolEnv(
                        id,
                        Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }
}
