use futures::FutureExt;
use pixi_git::resolver::RepositoryReference;

use super::{CommandDispatcherProcessor, NewDedupTask, TaskResult};
use crate::{
    CommandDispatcherError,
    command_dispatcher::{CommandDispatcherContext, GitCheckoutId, GitCheckoutTask},
    reporter::Reportable,
};

impl CommandDispatcherProcessor {
    /// Called when a [`ForegroundMessage::GitCheckout`] task was received.
    pub(crate) fn on_checkout_git(&mut self, task: GitCheckoutTask) {
        if self.is_parent_cancelled(task.parent) {
            return;
        }

        let repository_reference = RepositoryReference::from(&task.spec);

        let action =
            self.git_checkouts
                .on_task(repository_reference.clone(), task.tx, GitCheckoutId);
        let parent_reporter_context = task.parent.and_then(|ctx| self.reporter_context(ctx));

        let Some(NewDedupTask {
            id,
            cancellation_token,
            ..
        }) = Self::start_dedup_task(
            self,
            action,
            &task.spec,
            task.parent,
            task.cancellation_token,
            parent_reporter_context,
            CommandDispatcherContext::GitCheckout,
        )
        else {
            return;
        };

        if let Some(reporter_id) = self
            .git_checkout_reporters
            .get(&id)
            .and_then(|ids| ids.last().copied())
        {
            pixi_git::GitUrl::report_started(&self.reporter, reporter_id);
        }

        let resolver = self.inner.git_resolver.clone();
        let client = self.inner.download_client.clone();
        let cache_dir = self.inner.cache_dirs.git().clone();
        self.pending_futures.push(
            cancellation_token
                .run_until_cancelled_owned(async move {
                    resolver
                        .fetch(task.spec.clone(), client, cache_dir.into(), None)
                        .await
                        .map_err(CommandDispatcherError::Failed)
                })
                .map(move |result| {
                    TaskResult::GitCheckedOut(
                        id,
                        Box::new(result.unwrap_or(Err(CommandDispatcherError::Cancelled))),
                    )
                })
                .boxed_local(),
        );
    }
}
