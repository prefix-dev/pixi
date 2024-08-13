use crate::capabilities::{BackendCapabilities, FrontendCapabilities};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Parameters for the initialize request.
pub struct InitializeParams {
    /// The manifest that the build backend should use.
    pub manifest_path: PathBuf,
    /// The capabilities that the frontend provides.
    pub capabilities: FrontendCapabilities,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// The result of the initialize request.
pub struct InitializeResult {
    /// The capabilities that the backend provides.
    pub capabilities: BackendCapabilities,
}
