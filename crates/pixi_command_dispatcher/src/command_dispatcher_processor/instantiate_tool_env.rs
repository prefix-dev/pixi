use futures::FutureExt;

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
use crate::{
    CommandDispatcherError, Reporter,
    command_dispatcher::{CommandDispatcherContext, InstantiatedToolEnvId, Task},
    instantiate_tool_env::{
        InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult,
        InstantiateToolEnvironmentSpec,
    },
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

        let id = match &action {
            DedupAction::New { id, .. } | DedupAction::Subscribed { id, .. } => *id,
            DedupAction::AlreadyCompleted => return,
        };

        let dispatcher_context = CommandDispatcherContext::InstantiateToolEnv(id);

        if let DedupAction::New {
            cancellation_token,
            dedup_group_id,
            ..
        } = action
        {
            if let Some(parent) = task.parent {
                self.parent_contexts.insert(dispatcher_context, parent);
            }

            // Notify the reporter that a new task has been queued and started.
            let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
            let reporter_id = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_instantiate_tool_environment_reporter)
                .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

            if let Some(reporter_id) = reporter_id {
                self.instantiated_tool_envs_reporters
                    .entry(id)
                    .or_default()
                    .push(reporter_id);
            }

            if let Some((reporter, reporter_id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_instantiate_tool_environment_reporter)
                .zip(reporter_id)
            {
                reporter.on_started(reporter_id)
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
        } else if let DedupAction::Subscribed { dedup_group_id, .. } = action {
            // Notify the reporter for the subscriber as well.
            let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
            let reporter_id = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_instantiate_tool_environment_reporter)
                .map(|reporter| reporter.on_queued(parent_context, &task.spec, dedup_group_id));

            if let Some(reporter_id) = reporter_id {
                self.instantiated_tool_envs_reporters
                    .entry(id)
                    .or_default()
                    .push(reporter_id);
            }

            if let Some((reporter, reporter_id)) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_instantiate_tool_environment_reporter)
                .zip(reporter_id)
            {
                reporter.on_started(reporter_id)
            }
        }

        self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
    }

    /// Called when a [`TaskResult::InstantiateToolEnv`] task was received.
    pub(crate) fn on_instantiate_tool_environment_result(
        &mut self,
        id: InstantiatedToolEnvId,
        result: Result<
            InstantiateToolEnvironmentResult,
            CommandDispatcherError<InstantiateToolEnvironmentError>,
        >,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::InstantiateToolEnv(id));

        self.instantiated_tool_envs.on_result(id, result);
        if let Some(reporter_ids) = self.instantiated_tool_envs_reporters.remove(&id)
            && let Some(reporter) = self
                .reporter
                .as_deref_mut()
                .and_then(Reporter::as_instantiate_tool_environment_reporter)
        {
            for reporter_id in reporter_ids {
                reporter.on_finished(reporter_id);
            }
        }
    }
}
