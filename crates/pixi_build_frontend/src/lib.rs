mod build_frontend;
mod jsonrpc;
pub mod protocol;
mod protocols;

use std::fmt::{Debug, Formatter};

pub(crate) use protocols::{conda_build as conda_build_protocol, pixi as pixi_protocol};

mod protocol_builder;
mod tool;

use std::path::PathBuf;

pub use build_frontend::{BuildFrontend, BuildFrontendError};
use rattler_conda_types::MatchSpec;
use tokio::io::{AsyncRead, AsyncWrite};
pub use tool::{IsolatedToolSpec, SystemToolSpec, ToolSpec};
use url::Url;

pub use crate::protocol::Protocol;

#[derive(Debug)]
pub enum BackendOverride {
    /// Overrwide the backend with a specific tool.
    Spec(MatchSpec),

    /// Overwrite the backend with a specific tool.
    Path(PathBuf),

    /// Use the given IO for the backend.
    Io(InProcessBackend),
}

impl From<InProcessBackend> for BackendOverride {
    fn from(value: InProcessBackend) -> Self {
        Self::Io(value)
    }
}

impl From<InProcessBackend> for Option<BackendOverride> {
    fn from(value: InProcessBackend) -> Self {
        Some(value.into())
    }
}

/// A backend communication protocol that can run in the same process.
pub struct InProcessBackend {
    pub rpc_in: Box<dyn AsyncRead + Send + Sync + Unpin>,
    pub rpc_out: Box<dyn AsyncWrite + Send + Sync + Unpin>,
}

impl Debug for InProcessBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessBackend").finish()
    }
}

#[derive(Debug)]
pub struct SetupRequest {
    /// The source directory that contains the source package.
    pub source_dir: PathBuf,

    /// Overrides for the build tool.
    pub build_tool_override: Option<BackendOverride>,
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
