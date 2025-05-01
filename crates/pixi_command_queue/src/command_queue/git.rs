use pixi_git::{GitError, GitUrl, git::GitReference, source::Fetch};
use pixi_record::{PinnedGitCheckout, PinnedGitSpec, PinnedSourceSpec};
use pixi_spec::GitSpec;
use tokio::sync::oneshot;

use crate::{
    CommandQueue, CommandQueueError, SourceCheckout, SourceCheckoutError,
    command_queue::{CommandQueueContext, ForegroundMessage},
};

/// A task that is send to the background to checkout a git repository.
pub(crate) struct GitCheckoutTask {
    pub url: GitUrl,
    pub _context: Option<CommandQueueContext>,
    pub tx: oneshot::Sender<Result<Fetch, GitError>>,
}

impl CommandQueue {
    /// Check out the git repository associated with the given spec.
    pub async fn pin_and_checkout_git(
        &self,
        git_spec: GitSpec,
    ) -> Result<SourceCheckout, CommandQueueError<SourceCheckoutError>> {
        // Determine the git url, including the reference
        let git_reference = git_spec
            .rev
            .map(|rev| rev.into())
            .unwrap_or(GitReference::DefaultBranch);

        let git_url = GitUrl::try_from(git_spec.git)
            .map_err(GitError::from)
            .map_err(SourceCheckoutError::GitError)?
            .with_reference(git_reference.clone());

        // Fetch the git url in the background
        let fetch = self
            .checkout_git_url(git_url)
            .await
            .map_err(|err| err.map(SourceCheckoutError::from))?;

        // Determine the pinned spec from the fetch
        let pinned = PinnedGitSpec {
            git: fetch.repository().url.clone().into_url(),
            source: PinnedGitCheckout {
                commit: fetch.commit(),
                subdirectory: git_spec.subdirectory.clone(),
                reference: git_reference.into(),
            },
        };

        // Include any subdirectory
        let path = if let Some(subdir) = git_spec.subdirectory {
            fetch.path().join(subdir)
        } else {
            fetch.into_path()
        };

        Ok(SourceCheckout {
            path,
            pinned: PinnedSourceSpec::Git(pinned),
        })
    }

    /// Check out a particular git repository.
    ///
    /// The git checkout is performed in the background.
    pub async fn checkout_git_url(
        &self,
        git_url: GitUrl,
    ) -> Result<Fetch, CommandQueueError<GitError>> {
        let Some(sender) = self.channel.sender() else {
            // If this fails, it means the command_queue was dropped and the task is
            // immediately canceled.
            return Err(CommandQueueError::Cancelled);
        };

        // Send the task to the background and await the result.
        let (tx, rx) = oneshot::channel();
        sender
            .send(ForegroundMessage::GitCheckout(GitCheckoutTask {
                url: git_url,
                _context: self.context,
                tx,
            }))
            .map_err(|_| CommandQueueError::Cancelled)?;

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(CommandQueueError::Failed(err)),
            Err(_) => Err(CommandQueueError::Cancelled),
        }
    }
}
