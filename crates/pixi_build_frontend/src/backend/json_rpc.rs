use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::{
        ClientError,
        client::{ClientT, Error, SubscriptionClientT, TransportReceiverT, TransportSenderT},
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
        log_message::LogMessage,
        negotiate_capabilities::{NegotiateCapabilitiesParams, NegotiateCapabilitiesResult},
    },
};
use rattler_conda_types::VersionWithSource;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};

use super::{log_forwarder::LogForwarder, stderr::stream_stderr};
use crate::{
    backend::BackendOutputStream,
    error::BackendError,
    jsonrpc::{RpcParams, stdio_transport},
    tool::Tool,
};

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

/// The name of the most verbose level enabled by the current `tracing`
/// subscriber, in the format understood by the backend's
/// [`pixi_build_types::procedures::log_message::LOG_LEVEL_ENV`].
fn max_level_name(level: tracing::level_filters::LevelFilter) -> &'static str {
    use tracing::level_filters::LevelFilter;
    match level {
        LevelFilter::OFF => "off",
        LevelFilter::ERROR => "error",
        LevelFilter::WARN => "warn",
        LevelFilter::INFO => "info",
        LevelFilter::DEBUG => "debug",
        LevelFilter::TRACE => "trace",
    }
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
    /// The task that forwards `log/message` notifications from the backend
    /// into this process's `tracing` subscriber. Held so it is aborted when
    /// the backend is dropped.
    _log_forwarder: Option<AbortOnDrop>,
}

/// Aborts the wrapped task when dropped.
#[derive(Debug)]
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
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
            // Ask the backend to log at least as verbosely as we do; the
            // records it sends back over the log channel are filtered again
            // by our own subscriber.
            .env(
                procedures::log_message::LOG_LEVEL_ENV,
                max_level_name(tracing::level_filters::LevelFilter::current()),
            )
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
            // Log notifications can burst faster than the forwarder drains
            // them; jsonrpsee silently *unsubscribes* on overflow, so make
            // the buffer generous.
            .max_buffer_capacity_per_subscription(16 * 1024)
            .build_with_tokio(sender, receiver);

        // Register interest in `log/message` notifications before the first
        // request so no record can slip past unobserved. The backend only
        // starts sending them after we advertise `supports_log_messages`
        // below; the subscription simply stays idle for backends that never
        // send any.
        let log_forwarder = match client
            .subscribe_to_method::<LogMessage>(procedures::log_message::METHOD_NAME)
            .await
        {
            Ok(mut subscription) => Some(AbortOnDrop(tokio::spawn(async move {
                let mut forwarder = LogForwarder::new();
                while let Some(record) = subscription.next().await {
                    match record {
                        Ok(record) => forwarder.apply(record),
                        Err(err) => {
                            tracing::debug!(
                                "failed to parse a log/message notification from the build backend: {err}"
                            );
                        }
                    }
                }
            }))),
            Err(err) => {
                tracing::debug!(
                    "failed to subscribe to log/message notifications from the build backend: {err}"
                );
                None
            }
        };

        // Negotiate the capabilities with the backend.
        let negotiate_result: NegotiateCapabilitiesResult = client
            .request(
                procedures::negotiate_capabilities::METHOD_NAME,
                RpcParams::from(NegotiateCapabilitiesParams {
                    capabilities: FrontendCapabilities {
                        supports_log_messages: Some(log_forwarder.is_some()),
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
            _log_forwarder: log_forwarder,
        })
    }

    pub async fn conda_build_v1<W: BackendOutputStream + Send + 'static>(
        &self,
        request: CondaBuildV1Params,
        output_stream: W,
    ) -> Result<CondaBuildV1Result, CommunicationError> {
        // Capture all of stderr and stream it
        let stderr = self.stderr.as_ref().map(|stderr| {
            // Cancellation signal
            let (cancel_tx, cancel_rx) = oneshot::channel();
            // Spawn the stderr forwarding task
            let handle = tokio::spawn(stream_stderr(stderr.clone(), cancel_rx, output_stream));
            (cancel_tx, handle)
        });

        let result = self
            .client
            .request(
                procedures::conda_build_v1::METHOD_NAME,
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
                procedures::conda_build_v1::METHOD_NAME,
                self.manifest_path.parent().unwrap_or(&self.manifest_path),
                backend_output,
            )
        })
    }

    /// Call the `conda/outputs` method on the backend.
    pub async fn conda_outputs<W: BackendOutputStream + Send + 'static>(
        &self,
        request: CondaOutputsParams,
        output_stream: W,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        // Capture all of stderr and stream it
        let stderr = self.stderr.as_ref().map(|stderr| {
            // Cancellation signal
            let (cancel_tx, cancel_rx) = oneshot::channel();
            // Spawn the stderr forwarding task
            let handle = tokio::spawn(stream_stderr(stderr.clone(), cancel_rx, output_stream));
            (cancel_tx, handle)
        });

        let result = self
            .client
            .request(
                procedures::conda_outputs::METHOD_NAME,
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
                procedures::conda_outputs::METHOD_NAME,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    /// Captures every event re-emitted into the `tracing` subscriber,
    /// formatted as `LEVEL target [span:chain] message`.
    #[derive(Clone, Default)]
    struct Capture {
        events: StdArc<StdMutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for Capture
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            // Only capture re-emitted backend events; the jsonrpsee client
            // machinery under test emits its own events on this thread.
            if !event.metadata().target().starts_with("backend::") {
                return;
            }
            struct MessageVisitor(String);
            impl tracing::field::Visit for MessageVisitor {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    if field.name() == "message" {
                        self.0 = value.to_owned();
                    }
                }

                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);

            let scope = ctx
                .event_scope(event)
                .into_iter()
                .flat_map(|scope| scope.from_root())
                .map(|span| span.name().to_owned())
                .collect::<Vec<_>>()
                .join(":");
            self.events.lock().unwrap().push(format!(
                "{} {} [{scope}] {}",
                event.metadata().level(),
                event.metadata().target(),
                visitor.0
            ));
        }
    }

    type FakeBackendSender = crate::jsonrpc::Sender<tokio::io::WriteHalf<tokio::io::DuplexStream>>;
    type FakeBackendReceiver =
        crate::jsonrpc::Receiver<tokio::io::ReadHalf<tokio::io::DuplexStream>>;

    /// A fake build backend on the other end of an in-process transport. It
    /// answers `negotiateCapabilities` and `initialize`, records the
    /// frontend capabilities it received, and — when `log_records` is
    /// non-empty — pushes each of them as a `log/message` notification
    /// right after answering the negotiate request.
    fn spawn_fake_backend(
        log_records: Vec<serde_json::Value>,
    ) -> (
        FakeBackendSender,
        FakeBackendReceiver,
        StdArc<StdMutex<Option<serde_json::Value>>>,
    ) {
        let (frontend_end, backend_end) = tokio::io::duplex(64 * 1024);
        let (frontend_read, frontend_write) = tokio::io::split(frontend_end);
        let (backend_read, mut backend_write) = tokio::io::split(backend_end);

        let received_capabilities = StdArc::new(StdMutex::new(None));
        let received = received_capabilities.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(backend_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].clone();
                let mut responses = Vec::new();
                match request["method"].as_str().unwrap() {
                    "negotiateCapabilities" => {
                        *received.lock().unwrap() = Some(request["params"]["capabilities"].clone());
                        responses.push(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "capabilities": {
                                    "providesCondaOutputs": true,
                                    "providesCondaBuildV1": true,
                                }
                            }
                        }));
                        for record in &log_records {
                            responses.push(json!({
                                "jsonrpc": "2.0",
                                "method": "log/message",
                                "params": record,
                            }));
                        }
                    }
                    "initialize" => {
                        responses.push(json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                    }
                    other => panic!("unexpected method: {other}"),
                }
                for response in responses {
                    let mut line = response.to_string();
                    line.push('\n');
                    backend_write.write_all(line.as_bytes()).await.unwrap();
                }
            }
        });

        (
            crate::jsonrpc::Sender::from(frontend_write),
            crate::jsonrpc::Receiver::from(frontend_read),
            received_capabilities,
        )
    }

    async fn setup_backend_with_fake(
        sender: FakeBackendSender,
        receiver: FakeBackendReceiver,
    ) -> JsonRpcBackend {
        let dir = std::env::temp_dir();
        JsonRpcBackend::setup_with_transport(
            "fake-backend".to_string(),
            None,
            dir.clone(),
            dir.join("pixi.toml"),
            dir.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            sender,
            receiver,
            None,
        )
        .await
        .expect("setup should succeed")
    }

    /// Wait until the capture contains `count` events. Notifications arrive
    /// asynchronously with respect to the RPC responses.
    async fn wait_for_events(capture: &Capture, count: usize) {
        for _ in 0..500 {
            if capture.events.lock().unwrap().len() >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }

    #[tokio::test]
    async fn log_messages_are_re_emitted_through_the_tracing_subscriber() {
        let capture = Capture::default();
        let _guard = tracing_subscriber::registry()
            .with(capture.clone())
            .set_default();

        let (sender, receiver, received_capabilities) = spawn_fake_backend(vec![
            json!({
                "kind": "span_open",
                "id": 1,
                "level": "INFO",
                "target": "rattler_build_core::build",
                "name": "build",
            }),
            json!({
                "kind": "event",
                "level": "WARN",
                "target": "rattler_build_core::build",
                "message": "watch out",
                "span_id": 1,
            }),
            json!({"kind": "span_close", "id": 1}),
            json!({
                "kind": "event",
                "level": "ERROR",
                "target": "pixi_build_cmake::config",
                "message": "boom",
                "fields": {"code": 3},
            }),
        ]);
        let _backend = setup_backend_with_fake(sender, receiver).await;
        wait_for_events(&capture, 2).await;

        assert_eq!(
            *capture.events.lock().unwrap(),
            [
                "WARN backend::rattler_build_core::build [build] watch out",
                "ERROR backend::pixi_build_cmake::config [] boom code=3",
            ]
        );
        assert_eq!(
            received_capabilities
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|capabilities| capabilities["supportsLogMessages"].as_bool()),
            Some(true),
            "the frontend must advertise supports_log_messages"
        );
    }

    #[tokio::test]
    async fn backends_that_never_send_log_messages_work_unchanged() {
        let capture = Capture::default();
        let _guard = tracing_subscriber::registry()
            .with(capture.clone())
            .set_default();

        let (sender, receiver, _received) = spawn_fake_backend(Vec::new());
        let backend = setup_backend_with_fake(sender, receiver).await;
        assert!(backend.capabilities().provides_conda_build_v1());
        assert!(capture.events.lock().unwrap().is_empty());
    }
}
