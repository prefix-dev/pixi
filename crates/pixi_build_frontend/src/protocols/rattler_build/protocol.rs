use std::{path::PathBuf, sync::Arc};

use jsonrpsee::async_client::Client;
use pixi_build_types::BackendCapabilities;
use rattler_conda_types::ChannelConfig;
use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::Mutex,
};

use crate::protocols::BaseProtocol;

/// A protocol that uses a pixi manifest to invoke a build backend.
/// and uses a JSON-RPC client to communicate with the backend.
#[derive(Debug)]
pub struct Protocol {
    pub(super) _channel_config: ChannelConfig,
    pub(super) client: Client,

    /// A user friendly name for the backend.
    backend_identifier: String,

    pub(super) recipe_path: PathBuf,

    _backend_capabilities: BackendCapabilities,

    /// The build identifier
    build_id: usize,

    /// The handle to the stderr of the backend process.
    stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
}

impl BaseProtocol for Protocol {
    fn backend_identifier(&self) -> &str {
        &self.backend_identifier
    }

    fn client(&self) -> &Client {
        &self.client
    }

    fn build_id(&self) -> usize {
        self.build_id
    }

    fn stderr(&self) -> Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>> {
        self.stderr.clone()
    }

    fn new(
        client: Client,
        backend_identifier: String,
        channel_config: ChannelConfig,
        _source_dir: PathBuf,
        manifest_path: PathBuf,
        backend_capabilities: BackendCapabilities,
        build_id: usize,
        stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
    ) -> Self {
        Self {
            _channel_config: channel_config,
            client,
            backend_identifier,
            recipe_path: manifest_path,
            _backend_capabilities: backend_capabilities,
            build_id,
            stderr,
        }
    }

    fn manifests(&self) -> Vec<String> {
        self.recipe_path
            .to_str()
            .map(|s| s.to_string())
            .into_iter()
            .collect()
    }
}
