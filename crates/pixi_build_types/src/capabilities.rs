//! Capabilities that the frontend and backend provide.

use crate::PixiBuildApiVersion;
use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Capabilities that the backend provides.
pub struct BackendCapabilities {
    /// Whether the backend provides the ability for just conda metadata.
    pub provides_conda_metadata: Option<bool>,

    /// Whether the backend provides the ability to build conda packages.
    pub provides_conda_build: Option<bool>,

    /// The highest supported project model version.
    pub highest_supported_project_model: Option<u32>,

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
            provides_conda_metadata: Some(
                self.provides_conda_metadata() && expected.provides_conda_metadata(),
            ),
            provides_conda_build: Some(
                self.provides_conda_build_v0() && expected.provides_conda_build_v0(),
            ),
            highest_supported_project_model: Some(
                self.highest_supported_project_model()
                    .min(expected.highest_supported_project_model()),
            ),
            provides_conda_outputs: Some(
                self.provides_conda_outputs() && expected.provides_conda_outputs(),
            ),
            provides_conda_build_v1: Some(
                self.provides_conda_build_v1() && expected.provides_conda_build_v1(),
            ),
        }
    }

    /// Whether the backend provides the ability for just conda metadata.
    pub fn provides_conda_metadata(&self) -> bool {
        self.provides_conda_metadata.unwrap_or(false)
    }

    /// Whether the backend provides the ability to build conda packages.
    pub fn provides_conda_build_v0(&self) -> bool {
        self.provides_conda_build.unwrap_or(false)
    }

    /// The highest supported project model version.
    pub fn highest_supported_project_model(&self) -> u32 {
        self.highest_supported_project_model.unwrap_or(0)
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

#[derive(Debug, Serialize, Deserialize)]
/// Capabilities that the frontend provides.
pub struct FrontendCapabilities {}
