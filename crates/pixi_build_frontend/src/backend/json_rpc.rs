use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::{
        ClientError,
        client::{
            ClientT, Error, Subscription, SubscriptionClientT, TransportReceiverT, TransportSenderT,
        },
    },
    types::ErrorCode,
};
use miette::Diagnostic;
use ordermap::OrderMap;
use pixi_build_types::{
    BackendCapabilities, FrontendCapabilities, ProjectModel, TargetSelector,
    procedures::{
        self,
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
        initialize::{InitializeParams, InitializeResult},
        log::{self, LogParams},
        negotiate_capabilities::{NegotiateCapabilitiesParams, NegotiateCapabilitiesResult},
    },
};
use rattler_conda_types::VersionWithSource;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};

use super::output::{stream_logs, stream_stderr};
use crate::{
    backend::BackendOutputStream,
    error::BackendError,
    jsonrpc::{RpcParams, stdio_transport},
    tool::Tool,
};

/// How many log notifications to hold before dropping them.
///
/// Log events are only drained while a request is in flight, so this has to
/// absorb whatever a backend emits outside of one. jsonrpsee's default of 1024
/// is generous already; a build that outruns this is producing more log volume
/// than a user could read anyway.
const LOG_BUFFER_CAPACITY: usize = 4096;

#[derive(Debug, Error, Diagnostic)]
pub enum BuildBackendSetupError {
    #[error("an unexpected io error occurred while communicating with the pixi build backend")]
    Io(#[from] std::io::Error),

    #[error("the build backend executable '{0}' appears to be missing")]
    MissingExecutable(String),
}

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
    Communication(#[from] Box<CommunicationError>),
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

#[derive(Debug)]
pub struct JsonRpcBackend {
    /// The identifier of the backend.
    backend_identifier: String,
    /// The version of the backend.
    backend_version: Option<VersionWithSource>,
    /// The capabilities of the backend.
    backend_capabilities: BackendCapabilities,
    /// The JSON-RPC client to communicate with the backend.
    client: Client,
    /// The path to the manifest that is passed to the backend.
    manifest_path: PathBuf,
    /// The stderr of the backend process.
    stderr: Option<Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
    /// Structured log events the backend pushes over the connection. `None` if
    /// the backend never acknowledged the capability, in which case its log
    /// output arrives on stderr instead.
    logs: Option<Arc<Mutex<Subscription<LogParams>>>>,
}

#[allow(clippy::result_large_err)]
impl JsonRpcBackend {
    /// Set up a new protocol instance.
    /// This will spawn a new backend process and establish a JSON-RPC
    /// connection.
    #[allow(clippy::too_many_arguments)]
    pub async fn setup(
        source_dir: PathBuf,
        manifest_path: PathBuf,
        workspace_root: PathBuf,
        checkout_root: Option<PathBuf>,
        package_manifest: Option<ProjectModel>,
        configuration: Option<serde_json::Value>,
        target_configuration: Option<OrderMap<TargetSelector, serde_json::Value>>,
        cache_dir: Option<PathBuf>,
        workspace_scratch_directory: Option<PathBuf>,
        tool: Tool,
    ) -> Result<Self, InitializeError> {
        debug_assert!(source_dir.is_absolute());
        debug_assert!(manifest_path.is_absolute());
        debug_assert!(workspace_root.is_absolute());
        debug_assert!(checkout_root.as_ref().is_none_or(|p| p.is_absolute()));
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
            tool.version().cloned(),
            source_dir,
            manifest_path,
            workspace_root,
            checkout_root,
            package_manifest,
            configuration,
            target_configuration,
            cache_dir,
            workspace_scratch_directory,
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
        backend_version: Option<VersionWithSource>,
        source_dir: PathBuf,
        manifest_path: PathBuf,
        workspace_root: PathBuf,
        checkout_root: Option<PathBuf>,
        project_model: Option<ProjectModel>,
        configuration: Option<serde_json::Value>,
        target_configuration: Option<OrderMap<TargetSelector, serde_json::Value>>,
        cache_dir: Option<PathBuf>,
        workspace_scratch_directory: Option<PathBuf>,
        sender: impl TransportSenderT + Send,
        receiver: impl TransportReceiverT + Send,
        stderr: Option<Lines<BufReader<ChildStderr>>>,
    ) -> Result<Self, InitializeError> {
        let client: Client = ClientBuilder::default()
            // Set 24hours for request timeout because the backend may be long-running.
            .request_timeout(std::time::Duration::from_secs(86400))
            // Log events are only drained while a request is in flight, so the
            // buffer has to hold whatever a backend produces between requests.
            .max_buffer_capacity_per_subscription(LOG_BUFFER_CAPACITY)
            .build_with_tokio(sender, receiver);

        // Register interest in log notifications before announcing that we
        // understand them: the client drops notifications for methods that have
        // no handler, and the backend may start logging the moment it is told.
        let logs = match client
            .subscribe_to_method::<LogParams>(log::METHOD_NAME)
            .await
        {
            Ok(subscription) => Some(Arc::new(Mutex::new(subscription))),
            Err(err) => {
                // Not fatal: without the subscription the backend's log output
                // still reaches us over stderr.
                tracing::debug!("could not subscribe to backend log messages: {err}");
                None
            }
        };

        // Negotiate the capabilities with the backend.
        let negotiate_result: NegotiateCapabilitiesResult = client
            .request(
                procedures::negotiate_capabilities::METHOD_NAME,
                RpcParams::from(NegotiateCapabilitiesParams {
                    capabilities: FrontendCapabilities {
                        provides_log_notifications: Some(logs.is_some()),
                    },
                }),
            )
            .await
            .map_err(|err| {
                Box::new(CommunicationError::from_client_error(
                    backend_identifier.clone(),
                    err,
                    procedures::negotiate_capabilities::METHOD_NAME,
                    manifest_path.parent().unwrap_or(&manifest_path),
                    None,
                ))
            })?;

        // Invoke the initialize method on the backend to establish the connection.
        let _result: InitializeResult = client
            .request(
                procedures::initialize::METHOD_NAME,
                RpcParams::from(InitializeParams {
                    project_model,
                    configuration,
                    target_configuration,
                    manifest_path: manifest_path.clone(),
                    source_directory: Some(source_dir),
                    workspace_directory: Some(workspace_root),
                    checkout_root,
                    cache_directory: cache_dir,
                    workspace_scratch_directory,
                }),
            )
            .await
            .map_err(|err| {
                Box::new(CommunicationError::from_client_error(
                    backend_identifier.clone(),
                    err,
                    procedures::initialize::METHOD_NAME,
                    manifest_path.parent().unwrap_or(&manifest_path),
                    None,
                ))
            })?;

        Ok(Self {
            client,
            backend_identifier,
            backend_version,
            backend_capabilities: negotiate_result.capabilities,
            manifest_path,
            stderr: stderr.map(Mutex::new).map(Arc::new),
            logs,
        })
    }

    pub async fn conda_build_v1<W: BackendOutputStream + Send + 'static>(
        &self,
        request: CondaBuildV1Params,
        output_stream: W,
    ) -> Result<CondaBuildV1Result, CommunicationError> {
        self.call(
            procedures::conda_build_v1::METHOD_NAME,
            request,
            output_stream,
        )
        .await
    }

    /// Call the `conda/outputs` method on the backend.
    pub async fn conda_outputs<W: BackendOutputStream + Send + 'static>(
        &self,
        request: CondaOutputsParams,
        output_stream: W,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        self.call(
            procedures::conda_outputs::METHOD_NAME,
            request,
            output_stream,
        )
        .await
    }

    /// Call a long-running method, forwarding everything the backend emits
    /// while it runs to `output_stream`.
    async fn call<P, R, W>(
        &self,
        method: &str,
        request: P,
        output_stream: W,
    ) -> Result<R, CommunicationError>
    where
        P: Serialize + Send,
        R: DeserializeOwned,
        W: BackendOutputStream + Send + 'static,
    {
        let forwarding =
            OutputForwarding::start(self.stderr.as_ref(), self.logs.as_ref(), output_stream);

        let result = self.client.request(method, RpcParams::from(request)).await;

        let backend_output = forwarding.finish().await?;

        result.map_err(|err| {
            CommunicationError::from_client_error(
                self.backend_identifier.clone(),
                err,
                method,
                self.manifest_path.parent().unwrap_or(&self.manifest_path),
                backend_output,
            )
        })
    }

    /// Returns the backend identifier.
    pub fn identifier(&self) -> &str {
        &self.backend_identifier
    }

    /// Returns the version of the backend, if available.
    pub fn version(&self) -> Option<&VersionWithSource> {
        self.backend_version.as_ref()
    }

    /// Returns the advertised capabilities of the backend.
    pub fn capabilities(&self) -> &BackendCapabilities {
        &self.backend_capabilities
    }
}

/// Forwards everything a backend emits during one request to the caller's
/// output stream.
///
/// There are two sources and they are not redundant: structured log events
/// arrive over the connection, while stderr carries whatever the backend or a
/// subprocess it spawned wrote directly. Stderr is additionally buffered so a
/// backend that dies mid-request can be reported with its own output attached.
struct OutputForwarding {
    stderr: Option<Pump<Result<String, std::io::Error>>>,
    logs: Option<Pump<()>>,
}

/// A running forwarding task together with the signal that stops it.
struct Pump<T> {
    cancel: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<T>,
}

impl<T> Pump<T> {
    /// Stop the task and wait for what it produced.
    ///
    /// Returns `None` if the task was cancelled before producing anything. A
    /// panic inside the task is resumed here rather than swallowed, because it
    /// means a bug in the forwarding code itself.
    async fn stop(self) -> Option<T> {
        // A send error means the task already finished on its own.
        let _finished = self.cancel.send(());
        match self.task.await {
            Ok(value) => Some(value),
            Err(err) => match err.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(_cancelled) => None,
            },
        }
    }
}

#[allow(clippy::result_large_err)]
impl OutputForwarding {
    /// Start forwarding. Both sources write to the same stream, so they share it
    /// behind a lock rather than interleaving partial lines.
    fn start<W: BackendOutputStream + Send + 'static>(
        stderr: Option<&Arc<Mutex<Lines<BufReader<ChildStderr>>>>>,
        logs: Option<&Arc<Mutex<Subscription<LogParams>>>>,
        output_stream: W,
    ) -> Self {
        let sink = Arc::new(Mutex::new(output_stream));

        let stderr = stderr.map(|stderr| {
            let (cancel, cancel_rx) = oneshot::channel();
            let task = tokio::spawn(stream_stderr(stderr.clone(), cancel_rx, sink.clone()));
            Pump { cancel, task }
        });

        let logs = logs.map(|logs| {
            let (cancel, cancel_rx) = oneshot::channel();
            let task = tokio::spawn(stream_logs(logs.clone(), cancel_rx, sink.clone()));
            Pump { cancel, task }
        });

        Self { stderr, logs }
    }

    /// Stop forwarding and return whatever the backend wrote to stderr, which
    /// is what a premature exit is reported with.
    async fn finish(self) -> Result<Option<String>, CommunicationError> {
        if let Some(logs) = self.logs {
            logs.stop().await;
        }

        let Some(stderr) = self.stderr else {
            return Ok(None);
        };

        let lines = stderr
            .stop()
            .await
            .ok_or(CommunicationError::StdErrPipeStopped)?
            .map_err(|_| CommunicationError::StdErrPipeStopped)?;

        Ok(Some(lines))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};

    use super::JsonRpcBackend;
    use crate::{
        BackendOutputStream,
        jsonrpc::{Receiver, Sender},
    };

    /// Collects everything the frontend forwards, keeping the structure so the
    /// test can tell a notification apart from a rendered stderr line.
    #[derive(Clone, Default)]
    struct Collected {
        lines: Arc<StdMutex<Vec<String>>>,
        logs: Arc<StdMutex<Vec<pixi_build_types::procedures::log::LogParams>>>,
    }

    impl BackendOutputStream for Collected {
        fn on_line(&mut self, line: String) {
            self.lines.lock().expect("no panics in tests").push(line);
        }

        fn on_log(&mut self, log: pixi_build_types::procedures::log::LogParams) {
            self.logs.lock().expect("no panics in tests").push(log);
        }
    }

    /// A backend that answers the handshake, then emits log notifications
    /// before responding to `conda/outputs`.
    async fn fake_backend(
        requests: tokio::io::DuplexStream,
        mut responses: tokio::io::DuplexStream,
        negotiated: Arc<StdMutex<Option<serde_json::Value>>>,
    ) {
        let mut lines = TokioBufReader::new(requests).lines();
        while let Some(line) = lines.next_line().await.expect("the pipe stays open") {
            let request: serde_json::Value =
                serde_json::from_str(&line).expect("the frontend sends valid JSON");
            let id = request["id"].clone();

            let result = match request["method"].as_str().expect("requests name a method") {
                "negotiateCapabilities" => {
                    *negotiated.lock().expect("no panics in tests") =
                        Some(request["params"]["capabilities"].clone());
                    serde_json::json!({
                        "capabilities": {
                            "providesCondaOutputs": true,
                            "providesCondaBuildV1": true,
                        }
                    })
                }
                "initialize" => serde_json::json!({}),
                "conda/outputs" => {
                    // Two notifications ahead of the response: this is what the
                    // old stdio driver could not do.
                    for message in ["resolving variants", "found 1 output"] {
                        let notification = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "log/message",
                            "params": {
                                "level": "info",
                                "message": message,
                                "target": "fake_backend",
                                "fields": { "package": "libfoo" },
                            },
                        });
                        responses
                            .write_all(format!("{notification}\n").as_bytes())
                            .await
                            .expect("the pipe stays open");
                    }
                    serde_json::json!({ "outputs": [], "inputGlobs": [] })
                }
                other => panic!("the test backend was asked for {other}"),
            };

            let response = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
            responses
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("the pipe stays open");
        }
    }

    /// End to end over a real JSON-RPC connection: a backend that pushes log
    /// notifications mid-request reaches the caller as structured events, not
    /// as scraped text.
    #[tokio::test]
    async fn log_notifications_reach_the_output_stream() {
        let (frontend_out, backend_in) = tokio::io::duplex(64 * 1024);
        let (backend_out, frontend_in) = tokio::io::duplex(64 * 1024);

        let negotiated = Arc::new(StdMutex::new(None));
        tokio::spawn(fake_backend(backend_in, backend_out, negotiated.clone()));

        let backend = JsonRpcBackend::setup_with_transport(
            "fake".to_string(),
            None,
            std::env::current_dir().expect("a working directory"),
            std::env::current_dir()
                .expect("a working directory")
                .join("pixi.toml"),
            std::env::current_dir().expect("a working directory"),
            None,
            None,
            None,
            None,
            None,
            None,
            Sender::from(frontend_out),
            Receiver::from(frontend_in),
            None,
        )
        .await
        .expect("the handshake succeeds");

        assert_eq!(
            negotiated.lock().expect("no panics in tests").as_ref(),
            Some(&serde_json::json!({ "providesLogNotifications": true })),
            "the frontend must tell the backend it understands log notifications"
        );

        let collected = Collected::default();
        backend
            .conda_outputs(
                pixi_build_types::procedures::conda_outputs::CondaOutputsParams {
                    host_platform: rattler_conda_types::Platform::current(),
                    build_platform: rattler_conda_types::Platform::current(),
                    channels: Vec::new(),
                    variant_configuration: None,
                    variant_files: None,
                    work_directory: std::env::temp_dir(),
                },
                collected.clone(),
            )
            .await
            .expect("the fake backend answers");

        let logs = collected.logs.lock().expect("no panics in tests");
        assert_eq!(
            logs.len(),
            2,
            "both notifications must be delivered: {logs:?}"
        );
        assert_eq!(logs[0].message, "resolving variants");
        assert_eq!(logs[1].message, "found 1 output");
        assert_eq!(
            logs[0].target.as_deref(),
            Some("fake_backend"),
            "the target survives the trip, which stderr scraping could not do"
        );
        assert_eq!(
            logs[0].fields.get("package"),
            Some(&serde_json::json!("libfoo"))
        );
    }
}
