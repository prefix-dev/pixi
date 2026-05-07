//! Git checkout Key plus its reporter trait, semaphore, and cache
//! marker.

use std::sync::Arc;

use derive_more::Display;
use futures::future::Either;
use pixi_compute_cache_dirs::{CacheBase, CacheDirsExt, CacheLocation};
use pixi_compute_engine::{ComputeCtx, DataStore, Key};
use pixi_compute_network::HasDownloadClient;
use pixi_compute_reporters::{Active, LifecycleKind, OperationId, ReporterLifecycle};
use pixi_consts::consts;
use pixi_git::git::GitReference;
use pixi_git::resolver::RepositoryReference;
use pixi_git::source::Fetch as GitFetch;
use pixi_git::{GitError, GitUrl};
use pixi_record::{PinnedGitCheckout, PinnedGitSpec};
use pixi_spec::GitSpec;
use tokio::sync::Semaphore;

use crate::data::HasGitResolver;
use crate::{SourceCheckout, SourceCheckoutError};

/// [`CacheLocation`] marker for the cached git checkouts directory.
pub struct GitDir;
impl CacheLocation for GitDir {
    fn name() -> &'static str {
        consts::CACHED_GIT_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Root
    }
}

/// Per-key reporter for git checkouts.
pub trait GitCheckoutReporter: Send + Sync {
    fn on_queued(&self, env: &RepositoryReference) -> OperationId;
    fn on_started(&self, checkout_id: OperationId);
    fn on_finished(&self, checkout_id: OperationId);
}

/// Access the per-key git-checkout reporter from global data.
pub trait HasGitCheckoutReporter {
    fn git_checkout_reporter(&self) -> Option<&Arc<dyn GitCheckoutReporter>>;
}

impl HasGitCheckoutReporter for DataStore {
    fn git_checkout_reporter(&self) -> Option<&Arc<dyn GitCheckoutReporter>> {
        self.try_get::<Arc<dyn GitCheckoutReporter>>()
    }
}

/// Newtype around the semaphore that bounds concurrent git checkouts.
/// A distinct type lets [`DataStore`] key it independently from any
/// other `Arc<Semaphore>` registered alongside.
#[derive(Clone)]
pub struct GitCheckoutSemaphore(pub Arc<Semaphore>);

/// Access the semaphore bounding concurrent git checkouts. `None` means
/// "unlimited concurrency": the Key skips permit acquisition.
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
pub struct CheckoutGit(RepositoryReference);

impl CheckoutGit {
    pub fn new(git_url: &GitUrl) -> Self {
        Self(RepositoryReference::from(git_url))
    }
}

impl Key for CheckoutGit {
    type Value = Arc<Result<GitFetch, GitError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let cache_dir = ctx.cache_dir::<GitDir>().await;
        let data: &DataStore = ctx.global_data();
        let resolver = data.git_resolver().clone();
        let client = data.download_client().clone();
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

/// Per-spec git checkout entry points on [`ComputeCtx`].
pub trait GitSourceCheckoutExt {
    /// Check out the git repository associated with the given spec.
    fn pin_and_checkout_git(
        &mut self,
        git_spec: GitSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;

    /// Check out a pinned git repository at the given commit.
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
