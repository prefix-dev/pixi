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

    /// The path to the manifest relative to the source directory.
    relative_manifest_path: PathBuf,

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

    /// Returns the relative path from the source directory to the recipe.
    fn manifests(&self) -> Vec<String> {
        self.relative_manifest_path
            .to_str()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }

    fn new(
        client: Client,
        backend_identifier: String,
        channel_config: ChannelConfig,
        source_dir: PathBuf,
        manifest_path: PathBuf,
        backend_capabilities: BackendCapabilities,
        build_id: usize,
        stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
    ) -> Self {
        let relative_manifest_path = manifest_path
            .strip_prefix(source_dir)
            .unwrap_or(&manifest_path)
            .to_path_buf();
        Self {
            _channel_config: channel_config,
            client,
            backend_identifier,
            relative_manifest_path,
            _backend_capabilities: backend_capabilities,
            build_id,
            stderr,
        }
    }
}
