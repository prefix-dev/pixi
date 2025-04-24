mod backend_override;
mod build_frontend;
mod jsonrpc;
pub mod protocol;
mod protocols;

use std::fmt::{Debug, Formatter};

pub use protocols::JsonRPCBuildProtocol;
pub use protocols::builders::{pixi_protocol, rattler_build_protocol};

mod protocol_builder;
mod reporters;
pub mod tool;

use std::path::PathBuf;

pub use build_frontend::{BuildFrontend, BuildFrontendError};
pub use reporters::{CondaBuildReporter, CondaMetadataReporter};
pub use reporters::{NoopCondaBuildReporter, NoopCondaMetadataReporter};
use tokio::io::{AsyncRead, AsyncWrite};
pub use tool::{IsolatedToolSpec, SystemToolSpec, ToolContext, ToolSpec};
use url::Url;

pub use backend_override::BackendOverride;
pub use protocol_builder::EnabledProtocols;

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

    /// Identifier for the rest of the requests
    /// This is used to identify the requests that belong to the same build.
    pub build_id: usize,
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
