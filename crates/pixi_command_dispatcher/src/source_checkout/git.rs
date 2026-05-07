use crate::compute_data::{
    HasCacheDirs, HasDownloadClient, HasGitCheckoutReporter, HasGitResolver,
};
use crate::{GitCheckoutReporter, SourceCheckout, SourceCheckoutError};
use derive_more::Display;
use futures::future::Either;
use pixi_compute_engine::{ComputeCtx, DataStore, Key};
use pixi_compute_reporters::{Active, LifecycleKind, OperationId, ReporterLifecycle};
use pixi_git::git::GitReference;
use pixi_git::resolver::RepositoryReference;
use pixi_git::source::Fetch as GitFetch;
use pixi_git::{GitError, GitUrl};
use pixi_record::{PinnedGitCheckout, PinnedGitSpec};
use pixi_spec::GitSpec;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Newtype around the semaphore that bounds concurrent git checkouts.
/// Having a distinct type lets [`DataStore`] store it keyed by its own
/// `TypeId`, independent of any other `Arc<Semaphore>` on the store.
#[derive(Clone)]
pub struct GitCheckoutSemaphore(pub Arc<Semaphore>);

/// Access the semaphore bounding concurrent git checkouts.
///
/// Returns `None` when no semaphore was registered, which is treated as
/// "unlimited concurrency": the Key skips permit acquisition entirely.
pub trait HasGitCheckoutSemaphore {
    fn git_checkout_semaphore(&self) -> Option<&Arc<Semaphore>>;
}

impl HasGitCheckoutSemaphore for DataStore {
    fn git_checkout_semaphore(&self) -> Option<&Arc<Semaphore>> {
        self.try_get::<GitCheckoutSemaphore>().map(|s| &s.0)
    }
}

/// `LifecycleKind` for git checkouts.
struct GitReporterLifecycle;

impl LifecycleKind for GitReporterLifecycle {
    type Reporter<'r> = dyn GitCheckoutReporter + 'r;
    type Id = OperationId;
    type Env = RepositoryReference;

    fn queue<'r>(
        reporter: Option<&'r Self::Reporter<'r>>,
        env: &Self::Env,
    ) -> Option<Active<'r, Self::Reporter<'r>, Self::Id>> {
        reporter.map(|r| Active {
            reporter: r,
            id: r.on_queued(env),
        })
    }

    fn on_started<'r>(active: &Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_started(active.id);
    }

    fn on_finished<'r>(active: Active<'r, Self::Reporter<'r>, Self::Id>) {
        active.reporter.on_finished(active.id);
    }
}

/// Dedup key for a git checkout. Keyed on [`RepositoryReference`]
/// (normalized URL + reference, no `precise`) so callers that differ
/// only in whether they pre-resolved the commit still dedup.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{}@{}", _0.url.as_url(), _0.reference)]
pub(crate) struct CheckoutGit(RepositoryReference);

impl CheckoutGit {
    pub(crate) fn new(git_url: &GitUrl) -> Self {
        Self(RepositoryReference::from(git_url))
    }
}

impl Key for CheckoutGit {
    type Value = Arc<Result<GitFetch, GitError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let data: &DataStore = ctx.global_data();
        let resolver = data.git_resolver().clone();
        let client = data.download_client().clone();
        let cache_dir = data.cache_dirs().git();
        let semaphore = data.git_checkout_semaphore().cloned();
        let reporter = data.git_checkout_reporter().cloned();

        let lifecycle =
            ReporterLifecycle::<GitReporterLifecycle>::queued(reporter.as_deref(), &self.0);

        let _permit = match semaphore.as_ref() {
            Some(s) => Some(
                s.acquire()
                    .await
                    .expect("git checkout semaphore is never closed"),
            ),
            None => None,
        };
        let _lifecycle = lifecycle.start();

        // `from_reference` auto-fills `precise` from a full-commit
        // reference, so the resolver skips ref-resolution when it can.
        let git_url =
            GitUrl::from_reference(self.0.url.clone().into_url(), self.0.reference.clone());

        Arc::new(
            resolver
                .fetch(git_url, client, cache_dir.into_std_path_buf(), None)
                .await,
        )
    }
}

pub trait GitSourceCheckoutExt {
    /// Check out the git repository associated with the given spec.
    fn pin_and_checkout_git(
        &mut self,
        git_spec: GitSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;

    /// Checkout a pinned git repository.
    fn checkout_pinned_git(
        &mut self,
        git_spec: PinnedGitSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;
}

impl GitSourceCheckoutExt for ComputeCtx {
    fn pin_and_checkout_git(
        &mut self,
        git_spec: GitSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let git_reference = git_spec
            .rev
            .clone()
            .map(|rev| rev.into())
            .unwrap_or(GitReference::DefaultBranch);
        let pinned_git_reference = git_spec.rev.clone().unwrap_or_default();
        let subdirectory = git_spec.subdirectory.clone();

        let git_url = match GitUrl::try_from(git_spec.git).map_err(GitError::from) {
            Ok(url) => url.with_reference(git_reference),
            Err(e) => {
                return Either::Left(async move { Err(SourceCheckoutError::from(e)) });
            }
        };

        let fut = self.compute(&CheckoutGit::new(&git_url));
        Either::Right(async move {
            let fetch = fut.await.as_ref().clone()?;
            let pinned = PinnedGitSpec {
                git: fetch.repository().url.clone().into_url(),
                source: PinnedGitCheckout {
                    commit: fetch.commit(),
                    subdirectory,
                    reference: pinned_git_reference,
                },
            };
            Ok(SourceCheckout::from_git(fetch, pinned))
        })
    }

    fn checkout_pinned_git(
        &mut self,
        git_spec: PinnedGitSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let git_url = GitUrl::from_commit(
            git_spec.git.clone(),
            git_spec.source.reference.clone().into(),
            git_spec.source.commit,
        );
        let fut = self.compute(&CheckoutGit::new(&git_url));
        async move {
            let fetch = fut.await.as_ref().clone()?;
            Ok(SourceCheckout::from_git(fetch, git_spec))
        }
    }
}
