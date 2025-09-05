use std::collections::hash_map::Entry;

use futures::FutureExt;
use pixi_git::{GitError, resolver::RepositoryReference, source::Fetch};

use super::{CommandDispatcherProcessor, PendingGitCheckout, TaskResult};
use crate::{CommandDispatcherError, Reporter, command_dispatcher::GitCheckoutTask};

impl CommandDispatcherProcessor {
    /// Called when a [`ForegroundMessage::GitCheckout`] task was received.
    pub(crate) fn on_checkout_git(&mut self, task: GitCheckoutTask) {
        let parent_context = task.parent.and_then(|ctx| self.reporter_context(ctx));
        let repository_reference = RepositoryReference::from(&task.spec);
        match self.git_checkouts.entry(repository_reference.clone()) {
            Entry::Occupied(mut existing_checkout) => match existing_checkout.get_mut() {
                PendingGitCheckout::Pending(_, pending) => pending.push(task.tx),
                PendingGitCheckout::CheckedOut(fetch) => {
                    let _ = task.tx.send(Ok(fetch.clone()));
                }
                PendingGitCheckout::Errored => {
                    // Drop the sender, this will cause a cancellation on the other side.
                    drop(task.tx);
                }
            },
            Entry::Vacant(entry) => {
                // Notify the reporter that a new checkout has been queued.
                let reporter_id = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_git_reporter)
                    .map(|reporter| {
                        reporter.on_queued(parent_context, &RepositoryReference::from(&task.spec))
                    });

                entry.insert(PendingGitCheckout::Pending(reporter_id, vec![task.tx]));

                // Notify the reporter that the solve has started.
                if let Some((reporter, id)) = self
                    .reporter
                    .as_deref_mut()
                    .and_then(Reporter::as_git_reporter)
                    .zip(reporter_id)
                {
                    reporter.on_start(id)
                }

                let resolver = self.inner.git_resolver.clone();
                let client = self.inner.download_client.clone();
                let cache_dir = self.inner.cache_dirs.git().clone();
                self.pending_futures.push(
                    task.cancellation_token
                        .run_until_cancelled_owned(async move {
                            resolver
                                .fetch(task.spec.clone(), client, cache_dir, None)
                                .await
                                .map_err(CommandDispatcherError::Failed)
                        })
                        .map(|fetch| {
                            TaskResult::GitCheckedOut(
                                repository_reference,
                                fetch.unwrap_or(Err(CommandDispatcherError::Cancelled)),
                            )
                        })
                        .boxed_local(),
                );
            }
        }
    }

    /// Called when a git checkout task has completed.
    pub(crate) fn on_git_checked_out(
        &mut self,
        repository_reference: RepositoryReference,
        result: Result<Fetch, CommandDispatcherError<GitError>>,
    ) {
        let Some(PendingGitCheckout::Pending(reporter_id, pending)) =
            self.git_checkouts.get_mut(&repository_reference)
        else {
            unreachable!("cannot get a result for a git checkout that is not pending");
        };

        // Notify the reporter that the git checkout has finished.
        if let Some((reporter, id)) = self
            .reporter
            .as_deref_mut()
            .and_then(Reporter::as_git_reporter)
            .zip(*reporter_id)
        {
            reporter.on_finished(id)
        }

        match result {
            Ok(fetch) => {
                for tx in pending.drain(..) {
                    let _ = tx.send(Ok(fetch.clone()));
                }

                // Store the fetch in the git checkouts map.
                self.git_checkouts
                    .insert(repository_reference, PendingGitCheckout::CheckedOut(fetch));
            }
            Err(CommandDispatcherError::Failed(mut err)) => {
                // Only send the error to the first channel, drop the rest, which cancels them.
                for tx in pending.drain(..) {
                    match tx.send(Err(err)) {
                        Ok(_) => return,
                        Err(Err(failed_to_send)) => err = failed_to_send,
                        Err(Ok(_)) => unreachable!(),
                    }
                }

                self.git_checkouts
                    .insert(repository_reference, PendingGitCheckout::Errored);
            }
            Err(CommandDispatcherError::Cancelled) => {
                self.git_checkouts
                    .insert(repository_reference, PendingGitCheckout::Errored);
            }
        }
    }
}
