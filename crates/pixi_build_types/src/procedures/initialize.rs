//! This API was introduced in Pixi Build API version 0.

use std::path::PathBuf;

use ordermap::OrderMap;
use serde::{Deserialize, Serialize};

use crate::{TargetSelectorV1, VersionedProjectModel};

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
    ///
    /// This is an absolute path.
    pub manifest_path: PathBuf,

    /// The root directory of the source code that the build backend should use.
    /// If this is `None`, the backend should use the directory of the
    /// `manifest_path` as the source directory.
    ///
    /// This is an absolute path.
    pub source_dir: Option<PathBuf>,

    /// The root directory of the workspace.
    ///
    /// This is an absolute path.
    pub workspace_root: Option<PathBuf>,

    /// Optionally the cache directory to use for any caching activity.
    pub cache_directory: Option<PathBuf>,

    /// Project model that the backend should use even though it is an option
    /// it is highly recommended to use this field. Otherwise, it will be very
    /// easy to break backwards compatibility.
    pub project_model: Option<VersionedProjectModel>,

    /// Backend specific configuration passed from the frontend to the backend.
    pub configuration: Option<serde_json::Value>,

    /// Targets that apply to the backend.
    pub target_configuration: Option<OrderMap<TargetSelectorV1, serde_json::Value>>,
}

/// The result of the initialize request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {}
