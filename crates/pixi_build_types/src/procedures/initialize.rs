use crate::capabilities::{BackendCapabilities, FrontendCapabilities};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Parameters for the initialize request.
pub struct InitializeParams {
    /// The directory that the backend needs to operate on.
    pub source_dir: PathBuf,
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
