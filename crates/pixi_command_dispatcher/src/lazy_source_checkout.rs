use std::{cell::OnceCell, fmt::Display};

use pixi_record::PinnedSourceSpec;

use crate::{CommandDispatcher, CommandDispatcherError, SourceCheckout, SourceCheckoutError};

/// We don't always need a full source checkout, sometimes the pinned source
/// spec is enough. This type can be used to distinguish and convert between the
/// two.
#[derive(Debug, serde::Serialize, Clone)]
pub struct LazySourceCheckout {
    /// The pinned source spec, which is always available.
    spec: PinnedSourceSpec,

    /// The checked-out source, if it has been checked out.
    #[serde(skip)]
    checkout: OnceCell<SourceCheckout>,
}

impl Display for LazySourceCheckout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.spec)
    }
}

impl LazySourceCheckout {
    /// Returns the pinned source spec
    pub fn as_pinned(&self) -> &PinnedSourceSpec {
        &self.spec
    }

    /// Checkout the source if it is not already checked out.
    pub async fn checkout(
        &self,
        dispatcher: &CommandDispatcher,
    ) -> Result<&SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        if let Some(checkout) = self.checkout.get() {
            return Ok(checkout);
        }

        let checkout = dispatcher.checkout_pinned_source(self.spec.clone()).await?;
        Ok(self.checkout.get_or_init(|| checkout))
    }

    /// Converts this instance into a `SourceCheckout` if it has been checked
    /// out.
    pub fn into_checkout(self) -> Option<SourceCheckout> {
        self.checkout.into_inner()
    }
}
