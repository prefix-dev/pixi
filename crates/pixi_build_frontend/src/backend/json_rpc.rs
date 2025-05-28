use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::{
        ClientError,
        client::{ClientT, Error, TransportReceiverT, TransportSenderT},
    },
    types::ErrorCode,
};
use miette::Diagnostic;
use pixi_build_types::{
    FrontendCapabilities, ProjectModelV1, VersionedProjectModel, procedures,
    procedures::{
        conda_metadata::{CondaMetadataParams, CondaMetadataResult},
        initialize::{InitializeParams, InitializeResult},
        negotiate_capabilities::{NegotiateCapabilitiesParams, NegotiateCapabilitiesResult},
    },
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};

use crate::{
    error::BackendError,
    jsonrpc::{RpcParams, stdio_transport},
    protocols::{BuildBackendSetupError, stderr::stderr_buffer},
    tool::Tool,
};

/// An error that can occur when communicating with a build backend.
#[derive(Debug, Error, Diagnostic)]
pub enum CommunicationError {
    #[error("failed to communicate with the build backend ({0})")]
    #[diagnostic(help(
        "Ensure that the build backend implements the JSON-RPC protocol correctly."
    ))]
    JsonRpc(String, #[source] ClientError),
    #[error("the build backend ({0}) exited prematurely.\nBuild backend output:\n\n{1}")]
    PrematureExit(String, String),
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

#[derive(Debug, Error, Diagnostic)]
pub enum InitializeError {
    #[error("failed to setup communication with the build-backend")]
    #[diagnostic(help(
        "This is often caused by a broken build-backend. Try upgrading or downgrading the build backend."
    ))]
    Setup(
        #[diagnostic_source]
        #[from]
        BuildBackendSetupError,
    ),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Communication(#[from] CommunicationError),
}

impl CommunicationError {
    fn from_client_error(
        backend_identifier: String,
        err: ClientError,
        method: &str,
        root_dir: &Path,
        backend_output: Option<String>,
    ) -> Self {
        match err {
            Error::Call(err) if err.code() > -32001 => {
                Self::BackendError(BackendError::from_json_rpc(err, root_dir))
            }
            Error::Call(err) if err.code() == ErrorCode::MethodNotFound.code() => {
                Self::MethodNotImplemented(backend_identifier, method.to_string())
            }
            Error::RestartNeeded(_err) if backend_output.is_some() => Self::PrematureExit(
                backend_identifier,
                backend_output.expect("safe because checked above"),
            ),
            Error::ParseError(err) => Self::ParseError(backend_identifier, method.to_string(), err),
            e => Self::JsonRpc(backend_identifier, e),
        }
    }
}

pub struct JsonRpcBackend {
    /// The identifier of the backend.
    backend_identifier: String,
    /// The JSON-RPC client to communicate with the backend.
    client: Client,
    /// The path to the manifest that is passed to the backend.
    manifest_path: PathBuf,
    /// The stderr of the backend process.
    stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
}

impl JsonRpcBackend {
    /// Set up a new protocol instance.
    /// This will spawn a new backend process and establish a JSON-RPC
    /// connection.
    #[allow(clippy::too_many_arguments)]
    pub async fn setup(
        manifest_path: PathBuf,
        package_manifest: Option<ProjectModelV1>,
        configuration: Option<serde_json::Value>,
        cache_dir: Option<PathBuf>,
        tool: Tool,
    ) -> Result<Self, InitializeError> {
        // Spawn the tool and capture stdin/stdout.
        let command = tool.command();
        let program_name = command.get_program().to_string_lossy().into_owned();
        let mut process = match tokio::process::Command::from(command)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(process) => process,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(BuildBackendSetupError::MissingExecutable(program_name).into());
            }
            Err(err) => {
                return Err(BuildBackendSetupError::Io(err).into());
            }
        };

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
            manifest_path,
            package_manifest,
            configuration,
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
        manifest_path: PathBuf,
        project_model: Option<ProjectModelV1>,
        configuration: Option<serde_json::Value>,
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
        let _negotiate_result: NegotiateCapabilitiesResult = client
            .request(
                procedures::negotiate_capabilities::METHOD_NAME,
                RpcParams::from(NegotiateCapabilitiesParams {
                    capabilities: FrontendCapabilities {},
                }),
            )
            .await
            .map_err(|err| {
                CommunicationError::from_client_error(
                    backend_identifier.clone(),
                    err,
                    procedures::negotiate_capabilities::METHOD_NAME,
                    manifest_path.parent().unwrap_or(&manifest_path),
                    None,
                )
            })?;

        // Invoke the initialize method on the backend to establish the connection.
        let _result: InitializeResult = client
            .request(
                procedures::initialize::METHOD_NAME,
                RpcParams::from(InitializeParams {
                    project_model: project_model.map(VersionedProjectModel::V1),
                    configuration,
                    manifest_path: manifest_path.clone(),
                    cache_directory: cache_dir,
                }),
            )
            .await
            .map_err(|err| {
                CommunicationError::from_client_error(
                    backend_identifier.clone(),
                    err,
                    procedures::initialize::METHOD_NAME,
                    manifest_path.parent().unwrap_or(&manifest_path),
                    None,
                )
            })?;

        Ok(Self {
            client,
            backend_identifier,
            manifest_path,
            stderr: stderr.map(Mutex::new).map(Arc::new),
        })
    }

    /// Call the `conda/getMetadata` method on the backend.
    pub async fn conda_get_metadata(
        &self,
        request: &CondaMetadataParams,
    ) -> Result<CondaMetadataResult, CommunicationError> {
        // Capture all of stderr and discard it
        let stderr = self.stderr.as_ref().map(|stderr| {
            // Cancellation signal
            let (cancel_tx, cancel_rx) = oneshot::channel();
            // Spawn the stderr forwarding task
            let handle = tokio::spawn(stderr_buffer(stderr.clone(), cancel_rx));
            (cancel_tx, handle)
        });

        let result = self
            .client
            .request(
                procedures::conda_metadata::METHOD_NAME,
                RpcParams::from(request),
            )
            .await;

        // Wait for the stderr sink to finish, by signaling it to stop
        let backend_output = if let Some((cancel_tx, handle)) = stderr {
            // Cancel the stderr forwarding. Ignore any error because that means the
            // tasks also finished.
            let _err = cancel_tx.send(());
            let lines = handle.await.map_or_else(
                |e| match e.try_into_panic() {
                    Ok(panic) => std::panic::resume_unwind(panic),
                    Err(_) => Err(CommunicationError::StdErrPipeStopped),
                },
                |e| e.map_err(|_| CommunicationError::StdErrPipeStopped),
            )?;

            Some(lines)
        } else {
            None
        };

        result.map_err(|err| {
            CommunicationError::from_client_error(
                self.backend_identifier.clone(),
                err,
                procedures::conda_metadata::METHOD_NAME,
                self.manifest_path.parent().unwrap_or(&self.manifest_path),
                backend_output,
            )
        })
    }

    /// Returns the backend identifier.
    pub fn identifier(&self) -> &str {
        &self.backend_identifier
    }
}
