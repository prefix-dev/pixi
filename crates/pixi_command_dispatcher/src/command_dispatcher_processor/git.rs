use futures::FutureExt;
use pixi_git::{GitError, resolver::RepositoryReference, source::Fetch};

use super::CommandDispatcherProcessor;
use super::TaskResult;
use super::dedup::DedupAction;
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

        match self
            .git_checkouts
            .on_task(repository_reference.clone(), task.tx, GitCheckoutId)
        {
            DedupAction::AlreadyCompleted => {}
            DedupAction::New {
                cancellation_token,
                dedup_group_id,
                id,
                ..
            } => {
                let dispatcher_context = CommandDispatcherContext::GitCheckout(id);
                if let Some(parent) = task.parent {
                    self.parent_contexts.insert(dispatcher_context, parent);
                }

                // Notify the reporter.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = task.spec.report_queued(
                    &mut self.reporter,
                    parent_context,
                    Some(dedup_group_id),
                );

                if let Some(reporter_id) = reporter_id {
                    self.git_checkout_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                if let Some(reporter_id) = reporter_id {
                    pixi_git::GitUrl::report_started(&mut self.reporter, reporter_id);
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
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
            DedupAction::Subscribed {
                dedup_group_id, id, ..
            } => {
                let dispatcher_context = CommandDispatcherContext::GitCheckout(id);
                // Notify the reporter for the subscriber as well.
                let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
                let reporter_id = task.spec.report_queued(
                    &mut self.reporter,
                    parent_context,
                    Some(dedup_group_id),
                );

                if let Some(reporter_id) = reporter_id {
                    self.git_checkout_reporters
                        .entry(id)
                        .or_default()
                        .push(reporter_id);
                }

                if let Some(reporter_id) = reporter_id {
                    pixi_git::GitUrl::report_started(&mut self.reporter, reporter_id);
                }
                self.push_subscriber_monitor(dispatcher_context, task.cancellation_token);
            }
        };
    }

    /// Called when a git checkout task has completed.
    pub(crate) fn on_git_checked_out(
        &mut self,
        id: GitCheckoutId,
        result: Result<Fetch, CommandDispatcherError<GitError>>,
    ) {
        self.parent_contexts
            .remove(&CommandDispatcherContext::GitCheckout(id));

        let failed = result.is_err();
        self.git_checkouts.on_result(id, result);
        if let Some(reporter_ids) = self.git_checkout_reporters.remove(&id) {
            for reporter_id in reporter_ids {
                pixi_git::GitUrl::report_finished(&mut self.reporter, reporter_id, failed);
            }
        }
    }
}
