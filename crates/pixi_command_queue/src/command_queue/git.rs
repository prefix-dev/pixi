use pixi_git::{GitError, GitUrl, git::GitReference, source::Fetch};
use pixi_record::{PinnedGitCheckout, PinnedGitSpec, PinnedSourceSpec};
use pixi_spec::GitSpec;

use super::{Task, TaskSpec};
use crate::{CommandQueue, CommandQueueError, SourceCheckout, SourceCheckoutError};

/// A task that is send to the background to checkout a git repository.
pub(crate) type GitCheckoutTask = Task<GitUrl>;
impl TaskSpec for GitUrl {
    type Output = Fetch;
    type Error = GitError;
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
        self.execute_task(git_url).await
    }

    /// Checkout a pinned git repository.
    pub async fn checkout_pinned_git(
        &self,
        git_spec: PinnedGitSpec,
    ) -> Result<SourceCheckout, CommandQueueError<SourceCheckoutError>> {
        let git_url = GitUrl::from_commit(
            git_spec.git.clone(),
            git_spec.source.reference.clone().into(),
            git_spec.source.commit,
        );
        // Fetch the git url in the background
        let fetch = self
            .checkout_git_url(git_url)
            .await
            .map_err(|err| err.map(SourceCheckoutError::from))?;

        // Include any subdirectory
        let path = if let Some(subdir) = git_spec.source.subdirectory.as_ref() {
            fetch.path().join(subdir)
        } else {
            fetch.into_path()
        };

        Ok(SourceCheckout {
            path,
            pinned: PinnedSourceSpec::Git(git_spec),
        })
    }
}
