mod build_frontend;
mod jsonrpc;
pub mod protocol;
mod protocols;

pub(crate) use protocols::{conda_build as conda_build_protocol, pixi as pixi_protocol};

mod protocol_builder;
mod tool;

use std::path::PathBuf;

pub use build_frontend::{BuildFrontend, BuildFrontendError};
use rattler_conda_types::MatchSpec;
pub use tool::{IsolatedToolSpec, SystemToolSpec, ToolSpec};
use url::Url;

pub use crate::protocol::Protocol;

#[derive(Debug, Clone, Default)]
pub struct BackendOverrides {
    /// The specs to use for the build tool.
    pub spec: Option<MatchSpec>,

    /// Path to a system build tool.
    pub path: Option<PathBuf>,
}

#[derive(Debug)]
pub struct SetupRequest {
    /// The source directory that contains the source package.
    pub source_dir: PathBuf,

    /// Overrides for the build tool.
    pub build_tool_overrides: BackendOverrides,
}

#[derive(Debug)]
pub struct BuildOutput {
    /// Paths to the built artifacts.
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct CondaMetadataRequest {
    /// The base urls of the channels to use.
    pub channel_base_urls: Vec<Url>,
}
