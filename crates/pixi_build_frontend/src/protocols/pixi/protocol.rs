use std::{ffi::OsStr, path::PathBuf};

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{ClientT, Error, Error as ClientError, TransportReceiverT, TransportSenderT},
    types::ErrorCode,
};
use miette::{diagnostic, Diagnostic, IntoDiagnostic};
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
    jsonrpc::{stdio_transport, Receiver, RpcParams, Sender},
    protocols::error::BackendError,
    tool::Tool,
    CondaBuildReporter, CondaMetadataReporter,
};

#[derive(Debug, Error, Diagnostic)]
pub enum InitializeError {
    #[error("failed to setup communication with the build backend, an unexpected io error occurred while communicating with the pixi build backend")]
    #[diagnostic(help("Ensure that the project manifest contains a valid [build] section."))]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Protocol(#[from] ProtocolError),
}

impl ProtocolError {
    pub fn from_client_error(backend_identifier: String, err: ClientError, method: &str) -> Self {
        match err {
            Error::Call(err) if err.code() > -32001 => Self::BackendError(BackendError::from(err)),
            Error::Call(err) if err.code() == ErrorCode::MethodNotFound.code() => {
                Self::MethodNotImplemented(backend_identifier, method.to_string())
            }
            Error::ParseError(err) => Self::ParseError(backend_identifier, method.to_string(), err),
            e => Self::JsonRpc(backend_identifier, e),
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum ProtocolError {
    #[error("failed to communicate with the build backend ({0})")]
    #[diagnostic(help(
        "Ensure that the build backend implements the JSON-RPC protocol correctly."
    ))]
    JsonRpc(String, #[source] ClientError),
    #[error("received invalid response from the build backend ({0}) when calling '{1}'")]
    ParseError(String, String, #[source] serde_json::Error),
    #[error(transparent)]
    #[diagnostic(
        transparent,
        help("This error originates from the build backend specified in the project manifest.")
    )]
    BackendError(
        #[from]
        #[diagnostic_source]
        BackendError,
    ),
    #[error("the build backend ({0}) does not implement the method '{1}'")]
    #[diagnostic(help(
        "This is often caused by the build backend incorrectly reporting certain capabilities. Consider contacting the build backend maintainers for a fix."
    ))]
    MethodNotImplemented(String, String),
}

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

    /// Name of the project taken from the manifest
    project_name: Option<String>,

    _backend_capabilities: BackendCapabilities,
}

impl Protocol {
    pub(crate) async fn setup(
        source_dir: PathBuf,
        manifest_path: PathBuf,
        project_name: Option<String>,
        cache_dir: Option<PathBuf>,
        channel_config: ChannelConfig,
        tool: Tool,
    ) -> Result<Self, InitializeError> {
        match tool.try_into_executable() {
            Ok(tool) => {
                // Spawn the tool and capture stdin/stdout.
                let mut process = tokio::process::Command::from(tool.command())
                    .stdout(std::process::Stdio::piped())
                    .stdin(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit()) // TODO: Capture this?
                    .spawn()?;

                let backend_identifier = tool
                    .executable()
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .map_or_else(|| "<unknown>".to_string(), ToString::to_string);

                // Acquire the stdin/stdout handles.
                let stdin = process
                    .stdin
                    .take()
                    .expect("since we piped stdin we expect a valid value here");
                let stdout = process
                    .stdout
                    .expect("since we piped stdout we expect a valid value here");

                // Construct a JSON-RPC client to communicate with the backend process.
                let (tx, rx) = stdio_transport(stdin, stdout);
                Self::setup_with_transport(
                    backend_identifier,
                    source_dir,
                    manifest_path,
                    project_name,
                    cache_dir,
                    channel_config,
                    tx,
                    rx,
                )
                .await
            }
            Err(ipc) => {
                Self::setup_with_transport(
                    "<IPC>".to_string(),
                    source_dir,
                    manifest_path,
                    project_name,
                    cache_dir,
                    channel_config,
                    Sender::from(ipc.rpc_out),
                    Receiver::from(ipc.rpc_in),
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn setup_with_transport(
        backend_identifier: String,
        source_dir: PathBuf,
        manifest_path: PathBuf,
        project_name: Option<String>,
        cache_dir: Option<PathBuf>,
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
                    cache_directory: cache_dir,
                }),
            )
            .await
            .map_err(|err| {
                ProtocolError::from_client_error(
                    backend_identifier.clone(),
                    err,
                    procedures::initialize::METHOD_NAME,
                )
            })?;

        Ok(Self {
            backend_identifier,
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
                ProtocolError::from_client_error(
                    self.backend_identifier.clone(),
                    err,
                    procedures::conda_metadata::METHOD_NAME,
                )
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
                ProtocolError::from_client_error(
                    self.backend_identifier.clone(),
                    err,
                    procedures::conda_build::METHOD_NAME,
                )
            })
            .into_diagnostic();
        reporter.on_build_end(operation);
        result
    }

    /// Returns a unique identifier for the backend.
    pub fn backend_identifier(&self) -> &str {
        &self.backend_identifier
    }
}
