use std::collections::hash_map::Entry;

use futures::FutureExt;
use pixi_git::{GitError, GitUrl, resolver::RepositoryReference, source::Fetch};

use super::{CommandQueueProcessor, PendingGitCheckout, TaskResult};
use crate::command_queue::GitCheckoutTask;
use crate::Reporter;

impl CommandQueueProcessor {
    /// Called when a [`ForegroundMessage::GitCheckout`] task was received.
    pub(crate) fn on_checkout_git(&mut self, task: GitCheckoutTask) {
        match self.git_checkouts.entry(task.spec.clone()) {
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
                let reporter_id = self.reporter.as_deref_mut().and_then(Reporter::as_git_reporter).map(|reporter| {
                    reporter.on_checkout_queued(&RepositoryReference::from(&task.spec))
                });

                entry.insert(PendingGitCheckout::Pending(reporter_id, vec![task.tx]));

                // Notify the reporter that the solve has started.
                if let Some((reporter, id)) = self.reporter.as_deref_mut().and_then(Reporter::as_git_reporter).zip(reporter_id) {
                    reporter.on_checkout_start(id)
                }

                let resolver = self.inner.git_resolver.clone();
                let client = self.inner.download_client.clone();
                let cache_dir = self.inner.cache_dirs.root().clone();
                self.pending_futures.push(
                    async move {
                        let fetch = resolver
                            .fetch(task.spec.clone(), client, cache_dir, None)
                            .await;
                        TaskResult::GitCheckedOut(task.spec, fetch)
                    }
                    .boxed_local(),
                );
            }
        }
    }

    /// Called when a git checkout task has completed.
    pub(crate) fn on_git_checked_out(&mut self, url: GitUrl, result: Result<Fetch, GitError>) {
        let Some(PendingGitCheckout::Pending(reporter_id, pending)) =
            self.git_checkouts.get_mut(&url)
        else {
            unreachable!("cannot get a result for a git checkout that is not pending");
        };

        // Notify the reporter that the git checkout has finished.
        if let Some((reporter, id)) = self.reporter.as_deref_mut().and_then(Reporter::as_git_reporter).zip(*reporter_id) {
            reporter.on_checkout_finished(id)
        }

        match result {
            Ok(fetch) => {
                for tx in pending.drain(..) {
                    let _ = tx.send(Ok(fetch.clone()));
                }

                self.git_checkouts
                    .insert(url, PendingGitCheckout::CheckedOut(fetch));
            }
            Err(mut err) => {
                // Only send the error to the first channel, drop the rest, which cancels them.
                for tx in pending.drain(..) {
                    match tx.send(Err(err)) {
                        Ok(_) => return,
                        Err(Err(failed_to_send)) => err = failed_to_send,
                        Err(Ok(_)) => unreachable!(),
                    }
                }

                self.git_checkouts.insert(url, PendingGitCheckout::Errored);
            }
        }
    }
}
