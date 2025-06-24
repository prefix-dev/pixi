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
    pub fn new(spec: PinnedSourceSpec) -> Self {
        Self {
            spec,
            checkout: OnceCell::new(),
        }
    }

    /// Returns the pinned source spec
    pub fn as_pinned(&self) -> &PinnedSourceSpec {
        &self.spec
    }

    /// Checkout the source if it is not already checked out.
    pub async fn checkout(
        &mut self,
        dispatcher: &CommandDispatcher,
    ) -> Result<&SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        if let Some(checkout) = self.checkout.get() {
            return Ok(checkout);
        }

        let checkout = dispatcher.checkout_pinned_source(self.spec.clone()).await?;
        Ok(self.checkout.get_or_init(|| checkout))
    }

    /// Checkout the source and consume this instance.
    pub async fn into_checkout(
        self,
        dispatcher: &CommandDispatcher,
    ) -> Result<SourceCheckout, CommandDispatcherError<SourceCheckoutError>> {
        match self.checkout.into_inner() {
            Some(checkout) => Ok(checkout),
            None => dispatcher.checkout_pinned_source(self.spec.clone()).await,
        }
    }
}
