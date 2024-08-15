use std::path::PathBuf;

use futures::StreamExt;
use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::{
        __reexports::{
            serde_json,
            serde_json::{value::RawValue, Error},
        },
        client::{ClientT, ReceivedMessage, TransportReceiverT, TransportSenderT},
        traits::ToRpcParams,
    },
};
use miette::{Context, IntoDiagnostic};
use pixi_build_types::{
    procedures,
    procedures::initialize::{InitializeParams, InitializeResult},
    BackendCapabilities, FrontendCapabilities,
};
use rattler_conda_types::ChannelConfig;
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::{
    jsonrpc::{stdio_transport, RpcParams},
    tool::Tool,
    CondaMetadata, CondaMetadataRequest,
};

pub struct Protocol {
    pub(super) channel_config: ChannelConfig,
    pub(super) client: Client,

    backend_capabilities: BackendCapabilities,
}

impl Protocol {
    pub(crate) async fn new(
        manifest_path: PathBuf,
        channel_config: ChannelConfig,
        tool: Tool,
    ) -> miette::Result<Self> {
        // Spawn the tool and capture stdin/stdout.
        let process = tokio::process::Command::from(tool.command())
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit()) // TODO: Capture this?
            .spawn()
            .into_diagnostic()
            .context("failed to spawn pixi backend")?;

        // Acquire the stdin/stdout handles.
        let stdin = process
            .stdin
            .ok_or_else(|| miette::miette!("failed to open stdin for pixi backend"))?;
        let stdout = process
            .stdout
            .ok_or_else(|| miette::miette!("failed to open stdout for pixi backend"))?;

        // Construct a JSON-RPC client to communicate with the backend process.
        let (tx, rx) = stdio_transport(stdin, stdout);

        Self::new_with_transport(manifest_path, channel_config, tx, rx).await
    }

    pub async fn new_with_transport(
        manifest_path: PathBuf,
        channel_config: ChannelConfig,
        sender: impl TransportSenderT + Send,
        receiver: impl TransportReceiverT + Send,
    ) -> miette::Result<Self> {
        let client: Client = ClientBuilder::default()
            // Set 24hours for request timeout because the backend may be long-running.
            .request_timeout(std::time::Duration::from_secs(86400))
            .build_with_tokio(sender, receiver);

        // Invoke the initialize method on the backend to establish the connection.
        let result: InitializeResult = client
            .request(
                procedures::initialize::METHOD_NAME,
                RpcParams::from(InitializeParams {
                    manifest_path,
                    capabilities: FrontendCapabilities {},
                }),
            )
            .await
            .into_diagnostic()
            .context("failed to call 'initialize' on pixi backend")?;

        Ok(Self {
            channel_config,
            client,
            backend_capabilities: result.capabilities,
        })
    }

    /// Extract metadata from the recipe.
    pub async fn get_conda_metadata(
        &self,
        _request: &CondaMetadataRequest,
    ) -> miette::Result<CondaMetadata> {
        todo!("extract metadata from pixi manifest")
    }
}
