use crate::compute_data::{
    HasCacheDirs, HasDownloadClient, HasUrlCheckoutReporter, HasUrlResolver,
};
use crate::reporter::UrlCheckoutReporter;
use crate::reporter_lifecycle::{Active, LifecycleKind, ReporterLifecycle};
use crate::{SourceCheckout, SourceCheckoutError};
use derive_more::Display;
use pixi_compute_engine::{ComputeCtx, DataStore, Key};
use pixi_compute_reporters::OperationId;
use pixi_path::{AbsPathBuf, AbsPresumedDirPathBuf};
use pixi_record::{PinnedSourceSpec, PinnedUrlSpec};
use pixi_spec::UrlSpec;
use pixi_url::UrlError;
use std::sync::Arc;
use tokio::sync::Semaphore;
use url::Url;

/// Newtype around the semaphore that bounds concurrent URL archive
/// fetches. Having a distinct type lets [`DataStore`] store it keyed
/// by its own `TypeId`, independent of any other `Arc<Semaphore>` on
/// the store.
#[derive(Clone)]
pub struct UrlCheckoutSemaphore(pub Arc<Semaphore>);

/// Access the semaphore bounding concurrent URL archive fetches.
///
/// Returns `None` when no semaphore was registered, which is treated as
/// "unlimited concurrency": the Key skips permit acquisition entirely.
pub trait HasUrlCheckoutSemaphore {
    fn url_checkout_semaphore(&self) -> Option<&Arc<Semaphore>>;
}

impl HasUrlCheckoutSemaphore for DataStore {
    fn url_checkout_semaphore(&self) -> Option<&Arc<Semaphore>> {
        self.try_get::<UrlCheckoutSemaphore>().map(|s| &s.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlCheckout {
    pub pinned_url: PinnedUrlSpec,

    /// Directory which contains checkout.
    pub dir: AbsPresumedDirPathBuf,
}

impl UrlCheckout {
    pub fn into_path(self) -> AbsPresumedDirPathBuf {
        self.dir
    }
}

/// `LifecycleKind` for url checkouts.
struct UrlReporterLifecycle;

impl LifecycleKind for UrlReporterLifecycle {
    type Reporter<'r> = dyn UrlCheckoutReporter + 'r;
    type Id = OperationId;
    type Env = Url;

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

#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
pub(crate) struct CheckoutUrl(pub UrlSpec);

impl Key for CheckoutUrl {
    type Value = Arc<Result<UrlCheckout, UrlError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let data: &DataStore = ctx.global_data();
        let resolver = data.url_resolver().clone();
        let client = data.download_client().clone();
        let cache_dir = data.cache_dirs().url();
        let semaphore = data.url_checkout_semaphore().cloned();
        let reporter = data.url_checkout_reporter().cloned();

        let lifecycle =
            ReporterLifecycle::<UrlReporterLifecycle>::queued(reporter.as_deref(), &self.0.url);

        let _permit = match semaphore.as_ref() {
            Some(s) => Some(
                s.acquire()
                    .await
                    .expect("url checkout semaphore is never closed"),
            ),
            None => None,
        };
        let _lifecycle = lifecycle.start();

        Arc::new(
            resolver
                .fetch(self.0.clone(), client, cache_dir.into_std_path_buf(), None)
                .await
                .map(|fetch| UrlCheckout {
                    pinned_url: fetch.pinned().clone(),
                    dir: AbsPathBuf::new(fetch.path())
                        .expect("url fetch does not return absolute path")
                        .into_assume_dir(),
                }),
        )
    }
}

pub trait UrlSourceCheckoutExt {
    /// Check out the url associated with the given spec.
    fn pin_and_checkout_url(
        &mut self,
        url_spec: UrlSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;

    /// Checkout a pinned url.
    fn checkout_pinned_url(
        &mut self,
        pinned_url_spec: PinnedUrlSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;
}

impl UrlSourceCheckoutExt for ComputeCtx {
    fn pin_and_checkout_url(
        &mut self,
        url_spec: UrlSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let fut = self.compute(&CheckoutUrl(url_spec));
        async move {
            let UrlCheckout { pinned_url, dir } = fut.await.as_ref().clone()?;
            Ok(SourceCheckout {
                path: dir.into(),
                pinned: PinnedSourceSpec::Url(pinned_url),
            })
        }
    }

    fn checkout_pinned_url(
        &mut self,
        pinned_url_spec: PinnedUrlSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let url_spec = UrlSpec {
            url: pinned_url_spec.url.clone(),
            md5: pinned_url_spec.md5,
            sha256: Some(pinned_url_spec.sha256),
            subdirectory: pinned_url_spec.subdirectory.clone(),
        };
        let fut = self.compute(&CheckoutUrl(url_spec));
        async move {
            let fetch = fut.await.as_ref().clone()?;
            let path = if !pinned_url_spec.subdirectory.is_empty() {
                fetch
                    .dir
                    .join(pinned_url_spec.subdirectory.as_path())
                    .into_assume_dir()
            } else {
                fetch.into_path()
            };
            Ok(SourceCheckout {
                path: path.into(),
                pinned: PinnedSourceSpec::Url(pinned_url_spec),
            })
        }
    }
}
