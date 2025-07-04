//! Capabilities that the frontend and backend provide.

use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
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
}

#[derive(Debug, Serialize, Deserialize)]
/// Capabilities that the frontend provides.
pub struct FrontendCapabilities {}
