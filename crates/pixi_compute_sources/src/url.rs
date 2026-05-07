//! URL archive checkout Key plus its reporter trait, semaphore, and
//! cache marker.

use std::sync::Arc;

use derive_more::Display;
use pixi_compute_cache_dirs::{CacheBase, CacheDirsExt, CacheLocation};
use pixi_compute_engine::{ComputeCtx, DataStore, Key};
use pixi_compute_network::HasDownloadClient;
use pixi_compute_reporters::{Active, LifecycleKind, OperationId, ReporterLifecycle};
use pixi_consts::consts;
use pixi_path::{AbsPathBuf, AbsPresumedDirPathBuf};
use pixi_record::{PinnedSourceSpec, PinnedUrlSpec};
use pixi_spec::UrlSpec;
use pixi_url::UrlError;
use tokio::sync::Semaphore;
use url::Url;

use crate::data::HasUrlResolver;
use crate::{SourceCheckout, SourceCheckoutError};

/// [`CacheLocation`] marker for the cached URL-archive directory.
pub struct UrlDir;
impl CacheLocation for UrlDir {
    fn name() -> &'static str {
        consts::CACHED_URL_DIR
    }
    fn base() -> CacheBase {
        CacheBase::Root
    }
}

/// Per-key reporter for URL archive checkouts.
pub trait UrlCheckoutReporter: Send + Sync {
    fn on_queued(&self, env: &Url) -> OperationId;
    fn on_started(&self, checkout_id: OperationId);
    fn on_finished(&self, checkout_id: OperationId);
}

/// Access the per-key url-checkout reporter from global data.
pub trait HasUrlCheckoutReporter {
    fn url_checkout_reporter(&self) -> Option<&Arc<dyn UrlCheckoutReporter>>;
}

impl HasUrlCheckoutReporter for DataStore {
    fn url_checkout_reporter(&self) -> Option<&Arc<dyn UrlCheckoutReporter>> {
        self.try_get::<Arc<dyn UrlCheckoutReporter>>()
    }
}

/// Newtype around the semaphore that bounds concurrent URL archive
/// fetches. A distinct type lets [`DataStore`] key it independently
/// from any other `Arc<Semaphore>` registered alongside.
#[derive(Clone)]
pub struct UrlCheckoutSemaphore(pub Arc<Semaphore>);

/// Access the semaphore bounding concurrent URL archive fetches.
/// `None` means "unlimited concurrency": the Key skips permit
/// acquisition.
pub trait HasUrlCheckoutSemaphore {
    fn url_checkout_semaphore(&self) -> Option<&Arc<Semaphore>>;
}

impl HasUrlCheckoutSemaphore for DataStore {
    fn url_checkout_semaphore(&self) -> Option<&Arc<Semaphore>> {
        self.try_get::<UrlCheckoutSemaphore>().map(|s| &s.0)
    }
}

/// Result of a URL archive checkout: the pinned spec plus the
/// directory where the archive was extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlCheckout {
    pub pinned_url: PinnedUrlSpec,

    /// Directory containing the extracted archive.
    pub dir: AbsPresumedDirPathBuf,
}

impl UrlCheckout {
    pub fn into_path(self) -> AbsPresumedDirPathBuf {
        self.dir
    }
}

/// `LifecycleKind` for URL checkouts.
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

/// Dedup key for a URL archive checkout. Keyed on the full
/// [`UrlSpec`] so subdirectory variations dedup distinctly.
#[derive(Clone, Debug, Display, Hash, PartialEq, Eq)]
#[display("{_0}")]
pub struct CheckoutUrl(pub UrlSpec);

impl Key for CheckoutUrl {
    type Value = Arc<Result<UrlCheckout, UrlError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let cache_dir = ctx.cache_dir::<UrlDir>().await;
        let data: &DataStore = ctx.global_data();
        let resolver = data.url_resolver().clone();
        let client = data.download_client().clone();
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

/// Per-spec URL checkout entry points on [`ComputeCtx`].
pub trait UrlSourceCheckoutExt {
    /// Check out the URL archive associated with the given spec.
    fn pin_and_checkout_url(
        &mut self,
        url_spec: UrlSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;

    /// Check out a pinned URL archive at the recorded checksum.
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
