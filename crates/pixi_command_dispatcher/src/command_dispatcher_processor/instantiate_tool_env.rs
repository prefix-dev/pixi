use std::collections::hash_map::Entry;

use futures::FutureExt;

use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::{
    CommandDispatcherError, Reporter,
    command_dispatcher::{CommandDispatcherContext, InstantiatedToolEnvId, Task},
    instantiate_tool_env::{
        InstantiateToolEnvironmentError, InstantiateToolEnvironmentResult,
        InstantiateToolEnvironmentSpec,
    },
};

impl CommandDispatcherProcessor {
    /// Called when a [`super::ForegroundMessage::InstallPixiEnvironment`]
    /// task was received.
    pub(crate) fn on_instantiate_tool_environment(
        &mut self,
        task: Task<InstantiateToolEnvironmentSpec>,
    ) {
        let cache_key = task.spec.cache_key();
        let new_id = self.instantiated_tool_cache_keys.len();
        let id = *self
            .instantiated_tool_cache_keys
            .entry(cache_key)
            .or_insert_with(|| InstantiatedToolEnvId(new_id));

        if let Some(parent) = task.parent {
            // Store the parent context for the task.
            self.parent_contexts.insert(id.into(), parent);
        }

        match self.instantiated_tool_envs.entry(id) {
            Entry::Occupied(mut entry) => match entry.get_mut() {
                PendingDeduplicatingTask::Pending(pending, _) => {
                    pending.push(task.tx);
                }
                PendingDeduplicatingTask::Result(result, _) => {
                    let _ = task.tx.send(Ok(result.clone()));
                }
                PendingDeduplicatingTask::Errored => {
                    // Drop the sender, this will cause a cancellation on the other side.
                    drop(task.tx);
                }
            },
            Entry::Vacant(entry) => {
                entry.insert(PendingDeduplicatingTask::Pending(
                    vec![task.tx],
                    task.parent,
                ));

                // Notify the reporter that a new solve has been queued and started.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_instantiate_tool_environment_reporter)
                    .map(|reporter| reporter.on_queued(parent_context, &task.spec));

                if let Some(reporter_id) = reporter_id {
                    self.instantiated_tool_envs_reporters
                        .insert(id, reporter_id);
                }

                if let Some((reporter, reporter_id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_instantiate_tool_environment_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_started(reporter_id)
                }

                let command_queue = self.create_task_command_dispatcher(
                    CommandDispatcherContext::InstantiateToolEnv(id),
                );
                self.pending_futures.push(
                    task.cancellation_token
                        .run_until_cancelled_owned(task.spec.instantiate(command_queue))
                        .map(move |result| {
                            TaskResult::InstantiateToolEnv(
                                id,
                                result.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                            )
                        })
                        .boxed_local(),
                )
            }
        }
    }

    /// Called when a [`TaskResult::InstallPixiEnvironment`] task was
    /// received.
    ///
    /// This function will relay the result of the task back to the
    /// [`CommandDispatcher`] that issues it.
    pub(crate) fn on_instantiate_tool_environment_result(
        &mut self,
        id: InstantiatedToolEnvId,
        result: Result<
            InstantiateToolEnvironmentResult,
            CommandDispatcherError<InstantiateToolEnvironmentError>,
        >,
    ) {
        self.parent_contexts.remove(&id.into());
        if let Some((reporter, reporter_id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_instantiate_tool_environment_reporter)
            .zip(self.instantiated_tool_envs_reporters.remove(&id))
        {
            reporter.on_finished(reporter_id);
        }

        self.instantiated_tool_envs
            .get_mut(&id)
            .expect("cannot find instantiated tool env")
            .on_pending_result(result);
    }
}
