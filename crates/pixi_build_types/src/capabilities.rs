//! Capabilities that the frontend and backend provide.

use crate::PixiBuildApiVersion;
use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Capabilities that the backend provides.
pub struct BackendCapabilities {
    /// Whether the backend provides the `conda/outputs` API.
    pub provides_conda_outputs: Option<bool>,

    /// Whether the backend provides the `conda/build_v1` API.
    pub provides_conda_build_v1: Option<bool>,
}

impl BackendCapabilities {
    /// Mask the capabilities with the expected capabilities of a specific API version.
    pub fn mask_with_api_version(&self, version: &PixiBuildApiVersion) -> Self {
        let expected = version.expected_backend_capabilities();
        Self {
            provides_conda_outputs: Some(
                self.provides_conda_outputs() && expected.provides_conda_outputs(),
            ),
            provides_conda_build_v1: Some(
                self.provides_conda_build_v1() && expected.provides_conda_build_v1(),
            ),
        }
    }

    /// Whether the backend provides the `conda/outputs` API.
    pub fn provides_conda_outputs(&self) -> bool {
        self.provides_conda_outputs.unwrap_or(false)
    }

    /// Whether the backend provides the `conda/build_v1` API.
    pub fn provides_conda_build_v1(&self) -> bool {
        self.provides_conda_build_v1.unwrap_or(false)
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Capabilities that the frontend provides.
pub struct FrontendCapabilities {
    /// Whether the frontend accepts `log/message` notifications. Backends that
    /// talk to a frontend without this capability log to stderr instead.
    #[serde(default)]
    pub provides_log_notifications: Option<bool>,
}

impl FrontendCapabilities {
    /// Whether the frontend accepts `log/message` notifications.
    pub fn provides_log_notifications(&self) -> bool {
        self.provides_log_notifications.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::FrontendCapabilities;

    /// Frontends that predate `providesLogNotifications` send `{}`. A backend
    /// built against the newer types must still accept that and treat the
    /// capability as absent, otherwise upgrading the backend breaks every older
    /// pixi.
    #[test]
    fn capabilities_without_log_notifications_deserializes() {
        let capabilities: FrontendCapabilities =
            serde_json::from_str("{}").expect("an empty object is what older frontends send");

        assert_eq!(capabilities.provides_log_notifications, None);
        assert!(!capabilities.provides_log_notifications());
    }

    /// The field is `providesLogNotifications` on the wire, not the snake_case
    /// Rust name.
    #[test]
    fn capabilities_use_camel_case_on_the_wire() {
        let capabilities = FrontendCapabilities {
            provides_log_notifications: Some(true),
        };

        assert_eq!(
            serde_json::to_value(&capabilities).unwrap(),
            serde_json::json!({ "providesLogNotifications": true })
        );
    }
}
