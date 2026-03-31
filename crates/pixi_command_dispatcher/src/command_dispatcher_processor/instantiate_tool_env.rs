use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
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

        match self.instantiated_tool_envs.on_task(
            task.spec.cache_key(),
            task.tx,
            InstantiatedToolEnvId,
        ) {
            DedupAction::AlreadyCompleted => {}
            DedupAction::New {
                cancellation_token,
                dedup_group_id,
                id,
                ..
            } => {
                let dispatcher_context = CommandDispatcherContext::InstantiateToolEnv(id);
                if let Some(parent) = task.parent {
                    self.parent_contexts.insert(dispatcher_context, parent);
                }

                // Notify the reporter that a new task has been queued and started.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = task.spec.report_queued(
                    &mut self.reporter,
                    parent_context,
                    Some(dedup_group_id),
                );

                if let Some(reporter_id) = reporter_id {
                    self.instantiated_tool_envs_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                if let Some(reporter_id) = reporter_id {
                    InstantiateToolEnvironmentSpec::report_started(&mut self.reporter, reporter_id);
                }

                let command_queue = self.create_task_command_dispatcher(dispatcher_context);
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
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
            DedupAction::Subscribed {
                dedup_group_id, id, ..
            } => {
                let dispatcher_context = CommandDispatcherContext::InstantiateToolEnv(id);
                // Notify the reporter for the subscriber as well.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = task.spec.report_queued(
                    &mut self.reporter,
                    parent_context,
                    Some(dedup_group_id),
                );

                if let Some(reporter_id) = reporter_id {
                    self.instantiated_tool_envs_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                if let Some(reporter_id) = reporter_id {
                    InstantiateToolEnvironmentSpec::report_started(&mut self.reporter, reporter_id);
                }
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
        };
    }
}
