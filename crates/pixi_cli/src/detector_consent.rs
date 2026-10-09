//! How the CLI decides whether a virtual package detector without a stored
//! decision may run.

use std::sync::Arc;

use pixi_core::host::{DetectorConsent, NonInteractiveConsent};

/// The consent the CLI hands to host detection: detectors without a stored
/// decision are skipped with a warning that names the configuration key.
pub fn detector_consent() -> Arc<dyn DetectorConsent> {
    Arc::new(NonInteractiveConsent::default())
}
