use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::capabilities::{BackendCapabilities, FrontendCapabilities};

pub const METHOD_NAME: &str = "initialize";

/// Parameters for the initialize request.
///
/// This request is the first request that the frontend sends to the backend and
/// serves as a hand-shake between the two. The frontend provides its
/// capabilities which allows the backend to adapt its behavior to the frontend.
/// Conversely, the backend provides its capabilities in the response, which
/// allows the frontend to adapt its behavior to the capabilities of the
/// backend.
///
/// This request is the only request that requires a schema that is forever
/// backwards and forwards compatible. All other requests can be negotiated
/// through the capabilities structs. To facilitate this compatibility we keep
/// the number of arguments in this struct to a bare minimum.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// The manifest that the build backend should use.
    pub manifest_path: PathBuf,

    /// Additional configuration to configure the backend. This configuration is
    /// specific to the backend.
    pub configuration: serde_json::Value,

    /// The capabilities that the frontend provides.
    pub capabilities: FrontendCapabilities,

    /// Optionally the cache directory to use for any caching activity.
    pub cache_directory: Option<PathBuf>,
}

/// The result of the initialize request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// The capabilities that the backend provides.
    pub capabilities: BackendCapabilities,
}
