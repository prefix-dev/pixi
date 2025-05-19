use super::{CommandDispatcherProcessor, PendingDeduplicatingTask, TaskResult};
use crate::command_dispatcher::{InstantiatedToolEnvId, Task};
use crate::instantiate_tool_env::{
    InstantiateToolEnvironmentError, InstantiateToolEnvironmentSpec,
};
use crate::{CommandDispatcherError, command_dispatcher::CommandDispatcherContext};
use futures::FutureExt;
use rattler_conda_types::prefix::Prefix;
use std::collections::hash_map::Entry;

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
                    task.context,
                ));

                let command_queue = self
                    .create_task_command_queue(CommandDispatcherContext::InstantiateToolEnv(id));
                self.pending_futures.push(
                    task.spec
                        .instantiate(command_queue)
                        .map(move |result| TaskResult::InstantiateToolEnv(id, result))
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
        result: Result<Prefix, CommandDispatcherError<InstantiateToolEnvironmentError>>,
    ) {
        self.instantiated_tool_envs
            .get_mut(&id)
            .expect("cannot find instantiated tool env")
            .on_pending_result(result);
    }
}
