use std::path::PathBuf;

use pixi_record::{PinnedSourceSpec, PinnedUrlSpec};
use pixi_spec::UrlSpec;
pub use pixi_url::UrlError;

use super::{Task, TaskSpec};
use crate::{CommandDispatcher, CommandDispatcherError, SourceCheckout, SourceCheckoutError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlCheckout {
    pub pinned_url: PinnedUrlSpec,
    /// Directory which contains checkout.
    pub dir: PathBuf,
}

impl UrlCheckout {
    pub fn into_path(self) -> PathBuf {
        self.dir
    }
}

/// A task that is send to the background to checkout a url.
pub(crate) type UrlCheckoutTask = Task<UrlSpec>;
impl TaskSpec for UrlSpec {
    type Output = UrlCheckout;
    type Error = UrlError;
}

impl CommandDispatcher {
    /// Check out the url associated with the given spec.
    pub async fn pin_and_checkout_url(
        &self,
        url_spec: UrlSpec,
    ) -> Result<SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        // Fetch the url in the background
        let UrlCheckout { pinned_url, dir } = self
            .checkout_url(url_spec)
            .await
            .map_err(|err| err.map(SourceCheckoutError::from))?;

        Ok(SourceCheckout {
            path: dir,
            pinned: PinnedSourceSpec::Url(pinned_url),
        })
    }

    /// Check out a particular url.
    ///
    /// The url checkout is performed in the background.
    pub async fn checkout_url(
        &self,
        url: UrlSpec,
    ) -> Result<UrlCheckout, CommandDispatcherError<UrlError>> {
        self.execute_task(url).await
    }

    /// Checkout a pinned url.
    pub async fn checkout_pinned_url(
        &self,
        pinned_url_spec: PinnedUrlSpec,
    ) -> Result<SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        let url_spec = UrlSpec {
            url: pinned_url_spec.url.clone(),
            md5: pinned_url_spec.md5,
            sha256: Some(pinned_url_spec.sha256),
        };
        // Fetch the url in the background
        let fetch = self
            .checkout_url(url_spec)
            .await
            .map_err(|err| err.map(SourceCheckoutError::from))?;

        // TODO: Similar to TODO above.
        // let path = if let Some(subdir) = url_spec.source.subdirectory.as_ref() {
        //     fetch.path().join(subdir)
        // } else {
        //     fetch.into_path()
        // };
        let path = fetch.into_path();

        Ok(SourceCheckout {
            path,
            pinned: PinnedSourceSpec::Url(pinned_url_spec),
        })
    }
}
