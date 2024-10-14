use std::path::PathBuf;

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{ClientT, Error, Error as ClientError, TransportReceiverT, TransportSenderT},
    types::ErrorCode,
};
use miette::{Diagnostic, IntoDiagnostic};
use pixi_build_types::{
    procedures,
    procedures::{
        conda_build::{CondaBuildParams, CondaBuildResult},
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
    CondaBuildReporter, CondaMetadataReporter,
};

#[derive(Debug, Error)]
pub enum InitializeError {
    #[error("an unexpected io error occurred while communicating with the pixi build backend")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("failed to acquire stdin handle")]
    StdinHandle,
    #[error("failed to acquire stdout handle")]
    StdoutHandle,
}

impl ProtocolError {
    pub fn from_client_error(err: ClientError, method: &str) -> Self {
        match err {
            Error::Call(err) if err.code() > -32001 => Self::BackendError(BackendError::from(err)),
            Error::Call(err) if err.code() == ErrorCode::MethodNotFound.code() => {
                Self::MethodNotImplemented(method.to_string())
            }
            Error::ParseError(err) => Self::ParseError(method.to_string(), err),
            e => Self::JsonRpc(e),
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum ProtocolError {
    #[error(transparent)]
    JsonRpc(ClientError),
    #[error("received invalid response from backend when calling '{0}'")]
    ParseError(String, #[source] serde_json::Error),
    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendError(#[from] BackendError),
    #[error("the build backend does not implement the method '{0}'")]
    MethodNotImplemented(String),
}

/// A protocol that uses a pixi manifest to invoke a build backend.
/// and uses a JSON-RPC client to communicate with the backend.
pub struct Protocol {
    pub(super) _channel_config: ChannelConfig,
    pub(super) client: Client,

    /// The path to the manifest relative to the source directory.
    relative_manifest_path: PathBuf,

    /// Name of the project taken from the manifest
    project_name: Option<String>,

    _backend_capabilities: BackendCapabilities,
}

impl Protocol {
    pub(crate) async fn setup(
        source_dir: PathBuf,
        manifest_path: PathBuf,
        project_name: Option<String>,
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

        Self::setup_with_transport(
            source_dir,
            manifest_path,
            project_name,
            channel_config,
            tx,
            rx,
        )
        .await
    }

    pub async fn setup_with_transport(
        source_dir: PathBuf,
        manifest_path: PathBuf,
        project_name: Option<String>,
        channel_config: ChannelConfig,
        sender: impl TransportSenderT + Send,
        receiver: impl TransportReceiverT + Send,
    ) -> Result<Self, InitializeError> {
        let relative_manifest_path = manifest_path
            .strip_prefix(source_dir)
            .unwrap_or(&manifest_path)
            .to_path_buf();

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
            .map_err(|err| {
                ProtocolError::from_client_error(err, procedures::initialize::METHOD_NAME)
            })?;

        Ok(Self {
            _channel_config: channel_config,
            project_name,
            client,
            _backend_capabilities: result.capabilities,
            relative_manifest_path,
        })
    }

    /// Returns the relative path from the source directory to the recipe.
    pub fn manifests(&self) -> Vec<String> {
        self.relative_manifest_path
            .to_str()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Extract metadata from the recipe.
    pub async fn get_conda_metadata(
        &self,
        request: &CondaMetadataParams,
        reporter: &dyn CondaMetadataReporter,
    ) -> miette::Result<CondaMetadataResult> {
        let operation = reporter.on_metadata_start(self.project_name.as_deref().unwrap_or(""));
        let result = self
            .client
            .request(
                procedures::conda_metadata::METHOD_NAME,
                RpcParams::from(request),
            )
            .await
            .map_err(|err| {
                ProtocolError::from_client_error(err, procedures::conda_metadata::METHOD_NAME)
            })
            .into_diagnostic();
        reporter.on_metadata_end(operation);
        result
    }

    /// Build a specific conda package output
    pub async fn conda_build(
        &self,
        request: &CondaBuildParams,
        reporter: &dyn CondaBuildReporter,
    ) -> miette::Result<CondaBuildResult> {
        let operation = reporter.on_build_start(self.project_name.as_deref().unwrap_or(""));
        let result = self
            .client
            .request(
                procedures::conda_build::METHOD_NAME,
                RpcParams::from(request),
            )
            .await
            .map_err(|err| {
                ProtocolError::from_client_error(err, procedures::conda_build::METHOD_NAME)
            })
            .into_diagnostic();
        reporter.on_build_end(operation);
        result
    }
}
