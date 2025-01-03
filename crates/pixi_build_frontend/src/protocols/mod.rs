//! Implementations of the [`crate::Protocol`] type for various backends.

use std::{path::PathBuf, sync::Arc};

use error::BackendError;
use futures::TryFutureExt;
use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::{
        client::{ClientT, Error, TransportReceiverT, TransportSenderT},
        ClientError,
    },
    types::ErrorCode,
};

use crate::{
    jsonrpc::{stdio_transport, RpcParams},
    tool::Tool,
    CondaBuildReporter, CondaMetadataReporter,
};
use miette::Diagnostic;
use pixi_build_types::procedures::negotiate_capabilities::{
    NegotiateCapabilitiesParams, NegotiateCapabilitiesResult,
};
use pixi_build_types::{
    procedures::{
        self,
        conda_build::{CondaBuildParams, CondaBuildResult},
        conda_metadata::{CondaMetadataParams, CondaMetadataResult},
        initialize::{InitializeParams, InitializeResult},
    },
    BackendCapabilities, FrontendCapabilities,
};
use stderr::{stderr_null, stderr_stream};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::ChildStderr,
    sync::{oneshot, Mutex},
};

pub mod builders;
mod error;
pub(super) mod stderr;

#[derive(Debug, Error, Diagnostic)]
pub enum InitializeError {
    #[error("failed to setup communication with the build backend, an unexpected io error occurred while communicating with the pixi build backend")]
    #[diagnostic(help("Ensure that the project manifest contains a valid [build] section."))]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Protocol(#[from] ProtocolError),
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

    #[error("pipe of stderr stopped earlier than expected")]
    StdErrPipeStopped,
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

/// Protocol trait that is responsible for setting up and communicate with the backend.
/// This allows us to hide the JSON-RPC communication hidden in this protocol.
/// This protocol is generic over the manifest what are passed to the build backends.
/// This means that, for rattler-build, the manifest is a recipe.yaml file,
/// and for pixi it's a pixi.toml or a pyproject.toml file.
#[derive(Debug)]
pub struct JsonRPCBuildProtocol {
    backend_identifier: String,

    client: Client,

    build_id: usize,

    /// The directory that contains the source files.
    source_dir: PathBuf,

    /// The directory that contains the `recipe.yaml` or `pixi.toml` in the source directory.
    manifest_path: PathBuf,

    _backend_capabilities: BackendCapabilities,

    stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
}

impl JsonRPCBuildProtocol {
    /// Create a new instance of the protocol.
    #[allow(clippy::too_many_arguments)]
    fn new(
        client: Client,
        backend_identifier: String,
        source_dir: PathBuf,
        manifest_path: PathBuf,
        backend_capabilities: BackendCapabilities,
        build_id: usize,
        stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
    ) -> Self {
        Self {
            client,
            backend_identifier,
            source_dir,
            manifest_path,
            _backend_capabilities: backend_capabilities,
            build_id,
            stderr,
        }
    }

    /// Set up a new protocol instance.
    /// This will spawn a new backend process and establish a JSON-RPC connection.
    async fn setup(
        source_dir: PathBuf,
        manifest_path: PathBuf,
        configuration: serde_json::Value,
        build_id: usize,
        cache_dir: Option<PathBuf>,
        tool: Tool,
    ) -> Result<Self, InitializeError> {
        // Spawn the tool and capture stdin/stdout.
        let mut process = tokio::process::Command::from(tool.command())
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let backend_identifier = tool.executable().clone();

        // Acquire the stdin/stdout handles.
        let stdin = process
            .stdin
            .take()
            .expect("since we piped stdin we expect a valid value here");
        let stdout = process
            .stdout
            .expect("since we piped stdout we expect a valid value here");
        let stderr = process
            .stderr
            .map(|stderr| BufReader::new(stderr).lines())
            .expect("since we piped stderr we expect a valid value here");

        // Construct a JSON-RPC client to communicate with the backend process.
        let (tx, rx) = stdio_transport(stdin, stdout);
        Self::setup_with_transport(
            backend_identifier,
            source_dir,
            manifest_path,
            configuration,
            build_id,
            cache_dir,
            tx,
            rx,
            Some(stderr),
        )
        .await
    }

    /// Set up a new protocol instance with a given transport.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn setup_with_transport(
        backend_identifier: String,
        source_dir: PathBuf,
        // In case of rattler-build it's recipe.yaml
        manifest_path: PathBuf,
        configuration: serde_json::Value,
        build_id: usize,
        cache_dir: Option<PathBuf>,
        sender: impl TransportSenderT + Send,
        receiver: impl TransportReceiverT + Send,
        stderr: Option<Lines<BufReader<ChildStderr>>>,
    ) -> Result<Self, InitializeError> {
        let client: Client = ClientBuilder::default()
            // Set 24hours for request timeout because the backend may be long-running.
            .request_timeout(std::time::Duration::from_secs(86400))
            .build_with_tokio(sender, receiver);

        // Negotiate the capabilities with the backend.
        let negotiate_result: NegotiateCapabilitiesResult = client
            .request(
                procedures::negotiate_capabilities::METHOD_NAME,
                RpcParams::from(NegotiateCapabilitiesParams {
                    capabilities: FrontendCapabilities {},
                }),
            )
            .await
            .map_err(|err| {
                ProtocolError::from_client_error(
                    backend_identifier.clone(),
                    err,
                    procedures::negotiate_capabilities::METHOD_NAME,
                )
            })?;

        // TODO: select the correct protocol version based on the capabilities
        // Invoke the initialize method on the backend to establish the connection.
        let _result: InitializeResult = client
            .request(
                procedures::initialize::METHOD_NAME,
                RpcParams::from(InitializeParams {
                    manifest_path: manifest_path.clone(),
                    cache_directory: cache_dir,
                    configuration,
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

        Ok(JsonRPCBuildProtocol::new(
            client,
            backend_identifier,
            source_dir,
            manifest_path,
            negotiate_result.capabilities,
            build_id,
            stderr.map(Mutex::new).map(Arc::new),
        ))
    }

    /// Extract metadata from the recipe.
    pub async fn get_conda_metadata(
        &self,
        request: &CondaMetadataParams,
        reporter: &dyn CondaMetadataReporter,
    ) -> Result<CondaMetadataResult, ProtocolError> {
        // Capture all of stderr and discard it
        let stderr = self.stderr.as_ref().map(|stderr| {
            // Cancellation signal
            let (cancel_tx, cancel_rx) = oneshot::channel();
            // Spawn the stderr forwarding task
            let handle = tokio::spawn(stderr_null(stderr.clone(), cancel_rx));
            (cancel_tx, handle)
        });

        // Start the metadata operation
        let operation = reporter.on_metadata_start(self.build_id);

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
            });

        // Wait for the stderr sink to finish, by signaling it to stop
        if let Some((cancel_tx, handle)) = stderr {
            // Cancel the stderr forwarding
            if cancel_tx.send(()).is_err() {
                return Err(ProtocolError::StdErrPipeStopped);
            }
            handle.await.map_or_else(
                |e| match e.try_into_panic() {
                    Ok(panic) => std::panic::resume_unwind(panic),
                    Err(_) => Err(ProtocolError::StdErrPipeStopped),
                },
                |e| e.map_err(|_| ProtocolError::StdErrPipeStopped),
            )?;
        }

        reporter.on_metadata_end(operation);
        result
    }

    /// Build a specific conda package output
    pub async fn conda_build(
        &self,
        request: &CondaBuildParams,
        reporter: &dyn CondaBuildReporter,
    ) -> Result<CondaBuildResult, ProtocolError> {
        // Captures stderr output
        let stderr = self.stderr.as_ref().map(|stderr| {
            let (sender, receiver) = tokio::sync::mpsc::channel(100);
            let (cancel_tx, cancel_rx) = oneshot::channel();
            let handle = tokio::spawn(stderr_stream(stderr.clone(), sender, cancel_rx));
            (cancel_tx, receiver, handle)
        });

        let operation = reporter.on_build_start(self.build_id);
        let request = self
            .client
            .request(
                procedures::conda_build::METHOD_NAME,
                RpcParams::from(request),
            )
            .map_err(|err| {
                ProtocolError::from_client_error(
                    self.backend_identifier.clone(),
                    err,
                    procedures::conda_build::METHOD_NAME,
                )
            });

        // There can be two cases, the stderr is captured or is not captured
        // In the case of capturing we need to select between the request and the stderr
        // forwarding to drive these two futures concurrently
        //
        // In the other case we can just wait for the request to finish
        let result = if let Some((cancel_tx, receiver, handle)) = stderr {
            // This is the case where we capture stderr

            // Create a future that will forward stderr to the reporter
            let send_stderr = async {
                let mut receiver = receiver;
                while let Some(line) = receiver.recv().await {
                    reporter.on_build_output(operation, line);
                }
            };

            // Select between the request and the stderr forwarding
            let result = tokio::select! {
                result = request => result,
                _ = send_stderr => {
                    Err(ProtocolError::StdErrPipeStopped)
                }
            };

            // Cancel the stderr forwarding
            if cancel_tx.send(()).is_err() {
                return Err(ProtocolError::StdErrPipeStopped);
            }

            // Wait for the stderr forwarding to finish, it should because we cancelled
            handle.await.map_or_else(
                |e| match e.try_into_panic() {
                    Ok(panic) => std::panic::resume_unwind(panic),
                    Err(_) => Err(ProtocolError::StdErrPipeStopped),
                },
                |e| e.map_err(|_| ProtocolError::StdErrPipeStopped),
            )?;

            // Return the result
            result
        } else {
            // This is the case where we don't capture stderr
            request.await
        };

        // Build has completed
        reporter.on_build_end(operation);
        result
    }

    pub fn backend_identifier(&self) -> &str {
        &self.backend_identifier
    }

    pub fn manifests(&self) -> Vec<String> {
        self.manifest_path
            .strip_prefix(self.source_dir.clone())
            .unwrap_or(&self.manifest_path)
            .to_path_buf()
            .to_str()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }
}
