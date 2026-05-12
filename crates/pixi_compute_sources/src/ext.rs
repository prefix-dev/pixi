//! [`SourceCheckoutExt`]: per-spec checkout dispatch on
//! [`pixi_compute_engine::ComputeCtx`].

use futures::FutureExt;
use futures::future::BoxFuture;
use pixi_compute_engine::ComputeCtx;
use pixi_record::{PinnedPathSpec, PinnedSourceSpec};
use pixi_spec::{SourceLocationSpec, UrlSpec};

use crate::path::RootDirExt;
use crate::{GitSourceCheckoutExt, SourceCheckout, SourceCheckoutError, UrlSourceCheckoutExt};

/// Dispatch a checkout based on a [`SourceLocationSpec`] or a fully
/// pinned [`PinnedSourceSpec`].
pub trait SourceCheckoutExt {
    /// Resolve a source-location spec into a [`SourceCheckout`].
    ///
    /// - **Path** specs resolve against the workspace root, with `~/`
    ///   expansion and absolute-path passthrough.
    /// - **Git** specs clone or fetch the repository and check out the
    ///   referenced revision.
    /// - **URL** specs download and extract the archive.
    fn pin_and_checkout(
        &mut self,
        source_location_spec: SourceLocationSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;

    /// Like [`Self::pin_and_checkout`] but for a fully-pinned spec
    /// (e.g. a recorded git commit or URL checksum).
    fn checkout_pinned_source(
        &mut self,
        pinned_spec: PinnedSourceSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;
}

impl SourceCheckoutExt for ComputeCtx {
    fn pin_and_checkout(
        &mut self,
        source_location_spec: SourceLocationSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let fut: BoxFuture<'static, Result<SourceCheckout, SourceCheckoutError>> =
            match source_location_spec {
                SourceLocationSpec::Url(url) => self
                    .pin_and_checkout_url(UrlSpec {
                        url: url.url,
                        md5: url.md5,
                        sha256: url.sha256,
                        subdirectory: url.subdirectory,
                    })
                    .boxed(),
                SourceLocationSpec::Path(path) => {
                    let result = self.resolve_typed_path(path.path.to_path());
                    async move {
                        Ok(SourceCheckout {
                            path: result?,
                            pinned: PinnedSourceSpec::Path(PinnedPathSpec { path: path.path }),
                        })
                    }
                    .boxed()
                }
                SourceLocationSpec::Git(git_spec) => self.pin_and_checkout_git(git_spec).boxed(),
            };
        fut
    }

    fn checkout_pinned_source(
        &mut self,
        pinned_spec: PinnedSourceSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let fut: BoxFuture<'static, Result<SourceCheckout, SourceCheckoutError>> = match pinned_spec
        {
            PinnedSourceSpec::Path(path_spec) => {
                let path_result = self.resolve_typed_path(path_spec.path.to_path());
                async move {
                    Ok(SourceCheckout {
                        path: path_result?,
                        pinned: PinnedSourceSpec::Path(path_spec),
                    })
                }
                .boxed()
            }
            PinnedSourceSpec::Git(git_spec) => self.checkout_pinned_git(git_spec).boxed(),
            PinnedSourceSpec::Url(url_spec) => self.checkout_pinned_url(url_spec).boxed(),
        };
        fut
    }
}
