use std::path::PathBuf;

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{ClientT, Error, Error as ClientError, TransportReceiverT, TransportSenderT},
};
use miette::{Diagnostic, IntoDiagnostic};
use pixi_build_types::{
    procedures,
    procedures::{
        conda_metadata::{CondaMetadataParams, CondaMetadataResult},
        initialize::{InitializeParams, InitializeResult},
    },
    BackendCapabilities, FrontendCapabilities,
};
use rattler_conda_types::ChannelConfig;
use thiserror::Error;

use crate::{
    jsonrpc::{stdio_transport, RpcParams},
    protocols::error::BackendError,
    tool::Tool,
};

#[derive(Debug, Error)]
pub enum InitializeError {
    #[error("an unexpected io error occured while communicating with the pixi build backend")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    JsonRpc(ClientError),
    #[error(transparent)]
    BackendError(#[from] BackendError),
    #[error("failed to acquire stdin handle")]
    StdinHandle,
    #[error("failed to acquire stdout handle")]
    StdoutHandle,
}
impl From<ClientError> for InitializeError {
    fn from(value: ClientError) -> Self {
        match value {
            Error::Call(err) if err.code() > -32001 => Self::BackendError(BackendError::from(err)),
            e => Self::JsonRpc(e),
        }
    }
}

/// A protocol that uses a pixi manifest to invoke a build backend.
/// and uses a JSON-RPC client to communicate with the backend.
pub struct Protocol {
    pub(super) _channel_config: ChannelConfig,
    pub(super) client: Client,

    _backend_capabilities: BackendCapabilities,
}

impl Protocol {
    pub(crate) async fn setup(
        manifest_path: PathBuf,
        channel_config: ChannelConfig,
        tool: Tool,
    ) -> Result<Self, InitializeError> {
        // Spawn the tool and capture stdin/stdout.
        let process = tokio::process::Command::from(tool.command())
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit()) // TODO: Capture this?
            .spawn()?;

        // Acquire the stdin/stdout handles.
        let stdin = process.stdin.ok_or_else(|| InitializeError::StdinHandle)?;
        let stdout = process
            .stdout
            .ok_or_else(|| InitializeError::StdoutHandle)?;

        // Construct a JSON-RPC client to communicate with the backend process.
        let (tx, rx) = stdio_transport(stdin, stdout);

        Self::setup_with_transport(manifest_path, channel_config, tx, rx).await
    }

    pub async fn setup_with_transport(
        manifest_path: PathBuf,
        channel_config: ChannelConfig,
        sender: impl TransportSenderT + Send,
        receiver: impl TransportReceiverT + Send,
    ) -> Result<Self, InitializeError> {
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
            .await?;

        Ok(Self {
            _channel_config: channel_config,
            client,
            _backend_capabilities: result.capabilities,
        })
    }

    /// Extract metadata from the recipe.
    pub async fn get_conda_metadata(
        &self,
        request: &CondaMetadataParams,
    ) -> miette::Result<CondaMetadataResult> {
        self.client
            .request(
                procedures::conda_metadata::METHOD_NAME,
                RpcParams::from(request),
            )
            .await
            .map_err(|err| match err {
                Error::Call(err) if err.code() > -32001 => {
                    GetMetadataError::Backend(BackendError::from(err))
                }
                e => GetMetadataError::JsonRpc(e),
            })
            .into_diagnostic()
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum GetMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Backend(BackendError),

    #[error(transparent)]
    JsonRpc(#[from] ClientError),
}
