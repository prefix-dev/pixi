//! This API was introduced in Pixi Build API version 0.

use std::path::PathBuf;

use ordermap::OrderMap;
use serde::{Deserialize, Serialize};

use crate::{ProjectModel, TargetSelector};

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
    /// This is an absolute path to a manifest file.
    pub manifest_path: PathBuf,

    /// The root directory of the source code that the build backend should use.
    /// If this is `None`, the backend should use the directory of the
    /// `manifest_path` as the source directory.
    ///
    /// This is an absolute path. This is always a directory.
    pub source_directory: Option<PathBuf>,

    /// The root directory of the workspace.
    ///
    /// This is an absolute path.
    pub workspace_directory: Option<PathBuf>,

    /// Root of the git or url checkout this package was unpacked
    /// into, BEFORE any `subdirectory` is applied.  `None` for
    /// local-path sources (there is no checkout in that case;
    /// backends fall back to [`Self::workspace_directory`] for
    /// cross-package discovery).
    ///
    /// Distinct from [`Self::workspace_directory`] (which is anchored
    /// at a discovered pixi workspace) and from
    /// [`Self::source_directory`] (the package's own directory).
    /// Backends that need to reason about siblings inside the same
    /// remote checkout (e.g. ROS workspace sibling-package discovery)
    /// anchor their search here when it is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_root: Option<PathBuf>,

    /// Optionally the cache directory to use for any caching activity.
    pub cache_directory: Option<PathBuf>,

    /// Optional per-workspace scratch directory the backend may use to
    /// persist its own derived state across runs and across multiple
    /// backend instances within the same workspace. Pixi creates this
    /// directory and guarantees it survives across `pixi lock` and
    /// `pixi install` runs; it is removed by `pixi clean`.
    ///
    /// Convention: the backend creates one or more subdirectories
    /// inside, choosing names that avoid colliding with other
    /// backends. The name does not have to match the backend's own
    /// name; for example a backend may version its own cache layout
    /// by suffixing (`my-cache-v2/`). The backend owns invalidation;
    /// pixi will not stat or otherwise reason about anything stored
    /// here. Concurrent backend instances may run simultaneously
    /// against the same path, so writes should be atomic
    /// (write-tempfile-then-rename style).
    pub workspace_scratch_directory: Option<PathBuf>,

    /// Project model that the backend should use even though it is an option
    /// it is highly recommended to use this field. Otherwise, it will be very
    /// easy to break backwards compatibility.
    pub project_model: Option<ProjectModel>,

    /// Backend specific configuration passed from the frontend to the backend.
    pub configuration: Option<serde_json::Value>,

    /// Targets that apply to the backend.
    pub target_configuration: Option<OrderMap<TargetSelector, serde_json::Value>>,
}

/// The result of the initialize request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {}
