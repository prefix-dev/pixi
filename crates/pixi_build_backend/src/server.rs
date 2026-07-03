use std::{
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use fs_err::tokio as tokio_fs;
use jsonrpc_core::{Error, IoHandler, Params, serde_json, to_value};
use miette::{Context, IntoDiagnostic, JSONReportHandler};
use pixi_build_types::{
    ProjectModel,
    procedures::{
        self,
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
        initialize::InitializeParams,
        log_message::LogMessage,
        negotiate_capabilities::NegotiateCapabilitiesParams,
    },
};
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, RwLock, mpsc},
    task::JoinSet,
};

use crate::consts::DEBUG_OUTPUT_DIR;
use crate::protocol::{Protocol, ProtocolInstantiator};

/// A JSONRPC server that can be used to communicate with a client.
pub struct Server<T: ProtocolInstantiator> {
    instatiator: T,
    /// When set, `log/message` notifications received on this channel are
    /// forwarded to the client over stdout, and the flag is flipped once the
    /// frontend advertises `supports_log_messages` during capability
    /// negotiation.
    log_messages: Option<(mpsc::UnboundedReceiver<LogMessage>, Arc<AtomicBool>)>,
}

enum ServerState<T: ProtocolInstantiator> {
    /// Server has not been initialized yet.
    Uninitialized(T),
    /// Server has been initialized, with a protocol.
    Initialized(Box<dyn Protocol + Send + Sync + 'static>),
}

impl<T: ProtocolInstantiator> ServerState<T> {
    /// Convert to a protocol, if the server has been initialized.
    pub fn as_endpoint(
        &self,
    ) -> Result<&(dyn Protocol + Send + Sync + 'static), jsonrpc_core::Error> {
        match self {
            Self::Initialized(protocol) => Ok(protocol.as_ref()),
            _ => Err(Error::invalid_request()),
        }
    }
}

impl<T: ProtocolInstantiator> Server<T> {
    pub fn new(instatiator: T) -> Self {
        Self {
            instatiator,
            log_messages: None,
        }
    }

    /// Enables forwarding of structured log records to the client.
    ///
    /// Records received on `receiver` are sent to the client as
    /// `log/message` JSON-RPC notifications, but only after the frontend has
    /// advertised `supports_log_messages` during capability negotiation;
    /// `enabled` is flipped at that point so the producing side (the tracing
    /// layer) knows it can start emitting. Only supported when running over
    /// stdio.
    pub fn with_log_messages(
        mut self,
        receiver: mpsc::UnboundedReceiver<LogMessage>,
        enabled: Arc<AtomicBool>,
    ) -> Self {
        self.log_messages = Some((receiver, enabled));
        self
    }

    /// Run the server, communicating over stdin/stdout.
    ///
    /// Requests are read from stdin as line-delimited JSON and dispatched
    /// concurrently; responses and `log/message` notifications are written
    /// to stdout through a single writer task so their bytes never
    /// interleave.
    pub async fn run(self) -> miette::Result<()> {
        self.run_with_io(tokio::io::stdin(), tokio::io::stdout())
            .await
    }

    /// The implementation of [`Self::run`], generic over the input/output
    /// streams so tests can drive the server over in-process pipes.
    async fn run_with_io(
        mut self,
        input: impl tokio::io::AsyncRead + Unpin,
        mut output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    ) -> miette::Result<()> {
        let (log_messages, log_enabled) = match self.log_messages.take() {
            Some((receiver, enabled)) => (Some(receiver), Some(enabled)),
            None => (None, None),
        };
        let io = Arc::new(self.setup_io(log_enabled));

        // All bytes destined for the output flow through this channel.
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let mut writer = tokio::spawn(async move {
            while let Some(mut line) = out_rx.recv().await {
                line.push('\n');
                if output.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if output.flush().await.is_err() {
                    break;
                }
            }
        });

        // Forward structured log records as notifications.
        let log_task = log_messages.map(|mut receiver| {
            let out_tx = out_tx.clone();
            tokio::spawn(async move {
                while let Some(record) = receiver.recv().await {
                    let notification = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": procedures::log_message::METHOD_NAME,
                        "params": record,
                    });
                    if out_tx.send(notification.to_string()).is_err() {
                        break;
                    }
                }
            })
        });

        let mut requests = JoinSet::new();
        let mut stdin = BufReader::new(input).lines();
        while let Ok(Some(line)) = stdin.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let io = io.clone();
            let out_tx = out_tx.clone();
            requests.spawn(async move {
                if let Some(response) = io.handle_request(&line).await {
                    let _ = out_tx.send(response);
                }
            });
        }

        // Stdin closed: the client is gone. Finish in-flight requests, stop
        // the log forwarder, and flush any queued output before returning.
        while requests.join_next().await.is_some() {}
        if let Some(task) = log_task {
            task.abort();
            let _ = task.await;
        }
        drop(out_tx);
        let _ = (&mut writer).await;
        Ok(())
    }

    /// Run the server, communicating over HTTP.
    ///
    /// Notifications (and thus `log/message` forwarding) are not supported
    /// in this mode.
    pub fn run_over_http(self, port: u16) -> miette::Result<()> {
        let io = self.setup_io(None);
        jsonrpc_http_server::ServerBuilder::new(io)
            .start_http(&SocketAddr::from(([127, 0, 0, 1], port)))
            .into_diagnostic()?
            .wait();
        Ok(())
    }

    /// Setup the IO inner handler.
    fn setup_io(self, log_enabled: Option<Arc<AtomicBool>>) -> IoHandler {
        // Construct a server
        let mut io = IoHandler::new();
        io.add_method(
            procedures::negotiate_capabilities::METHOD_NAME,
            move |params: Params| {
                let log_enabled = log_enabled.clone();
                async move {
                    let params: NegotiateCapabilitiesParams = params.parse()?;
                    // Only start emitting `log/message` notifications once the
                    // frontend has told us it understands them.
                    if params.capabilities.supports_log_messages == Some(true)
                        && let Some(enabled) = log_enabled
                    {
                        enabled.store(true, Ordering::Release);
                    }
                    let result = T::negotiate_capabilities(params)
                        .await
                        .map_err(convert_error)?;
                    Ok(to_value(result).expect("failed to convert to json"))
                }
            },
        );

        let project_model = Arc::new(Mutex::new(None));

        let state = Arc::new(RwLock::new(ServerState::Uninitialized(self.instatiator)));
        let initialize_state = state.clone();
        let initialize_project_model = project_model.clone();
        io.add_method(
            procedures::initialize::METHOD_NAME,
            move |params: Params| {
                let pm = initialize_project_model.clone();
                let state = initialize_state.clone();

                async move {
                    let params: InitializeParams = params.parse()?;

                    if let Some(project_model) = &params.project_model {
                        let mut lock = pm.lock().await;
                        *lock = Some(project_model.clone());
                    }

                    let mut state = state.write().await;
                    let ServerState::Uninitialized(initializer) = &mut *state else {
                        return Err(Error::invalid_request());
                    };

                    let (protocol_endpoint, result) = initializer
                        .initialize(params)
                        .await
                        .map_err(convert_error)?;
                    *state = ServerState::Initialized(protocol_endpoint);

                    Ok(to_value(result).expect("failed to convert to json"))
                }
            },
        );

        let conda_outputs = state.clone();
        let conda_outputs_project_model = project_model.clone();
        io.add_method(
            procedures::conda_outputs::METHOD_NAME,
            move |params: Params| {
                let pm = conda_outputs_project_model.clone();
                let state = conda_outputs.clone();

                async move {
                    let params: CondaOutputsParams = params.parse()?;
                    let state = state.read().await;
                    let endpoint = state.as_endpoint()?;

                    let debug_dir = params.work_directory.join(DEBUG_OUTPUT_DIR);

                    if let Some(project_model) = pm.lock().await.take() {
                        log_project_model(&debug_dir, project_model)
                            .await
                            .map_err(convert_error)?;
                    }

                    log_conda_outputs(&debug_dir, &params)
                        .await
                        .map_err(convert_error)?;

                    match endpoint.conda_outputs(params).await {
                        Ok(result) => {
                            log_conda_outputs_response(&debug_dir, &result)
                                .await
                                .map_err(convert_error)?;

                            Ok(to_value(result).expect("failed to convert to json"))
                        }
                        Err(err) => {
                            let json_error = convert_error(err);
                            log_conda_outputs_error(&debug_dir, &json_error)
                                .await
                                .map_err(convert_error)?;
                            Err(json_error)
                        }
                    }
                }
            },
        );

        let conda_build_v1 = state.clone();
        let conda_build_project_model = project_model.clone();
        io.add_method(
            procedures::conda_build_v1::METHOD_NAME,
            move |params: Params| {
                let pm = conda_build_project_model.clone();
                let state = conda_build_v1.clone();

                async move {
                    let params: CondaBuildV1Params = params.parse()?;
                    let state = state.read().await;
                    let endpoint = state.as_endpoint()?;

                    let debug_dir = params.work_directory.join(DEBUG_OUTPUT_DIR);

                    if let Some(project_model) = pm.lock().await.take() {
                        log_project_model(&debug_dir, project_model)
                            .await
                            .map_err(convert_error)?;
                    }

                    log_conda_build_v1(&debug_dir, &params)
                        .await
                        .map_err(convert_error)?;

                    match endpoint.conda_build_v1(params).await {
                        Ok(result) => {
                            log_conda_build_v1_response(&debug_dir, &result)
                                .await
                                .map_err(convert_error)?;

                            Ok(to_value(result).expect("failed to convert to json"))
                        }
                        Err(err) => {
                            let json_error = convert_error(err);
                            log_conda_build_v1_error(&debug_dir, &json_error)
                                .await
                                .map_err(convert_error)?;
                            Err(json_error)
                        }
                    }
                }
            },
        );

        io
    }
}

fn convert_error(err: miette::Report) -> jsonrpc_core::Error {
    let rendered = JSONReportHandler::new();
    let mut json_str = String::new();
    rendered
        .render_report(&mut json_str, err.as_ref())
        .expect("failed to convert error to json");
    let data = serde_json::from_str(&json_str).expect("failed to parse json error");
    jsonrpc_core::Error {
        code: jsonrpc_core::ErrorCode::ServerError(-32000),
        message: err.to_string(),
        data: Some(data),
    }
}

async fn log_project_model(debug_dir: &Path, project_model: ProjectModel) -> miette::Result<()> {
    write_json_file(debug_dir, "project_model.json", &project_model).await
}

async fn log_conda_outputs(debug_dir: &Path, params: &CondaOutputsParams) -> miette::Result<()> {
    write_json_file(debug_dir, "conda_outputs_params.json", params).await
}

async fn log_conda_outputs_response(
    debug_dir: &Path,
    result: &CondaOutputsResult,
) -> miette::Result<()> {
    write_json_file(debug_dir, "conda_outputs_response.json", result).await
}

async fn log_conda_outputs_error(
    debug_dir: &Path,
    error: &jsonrpc_core::Error,
) -> miette::Result<()> {
    write_json_file(debug_dir, "conda_outputs_error.json", error).await
}

async fn log_conda_build_v1(debug_dir: &Path, params: &CondaBuildV1Params) -> miette::Result<()> {
    write_json_file(debug_dir, "conda_build_v1_params.json", params).await
}

async fn log_conda_build_v1_response(
    debug_dir: &Path,
    result: &CondaBuildV1Result,
) -> miette::Result<()> {
    write_json_file(debug_dir, "conda_build_v1_response.json", result).await
}

async fn log_conda_build_v1_error(
    debug_dir: &Path,
    error: &jsonrpc_core::Error,
) -> miette::Result<()> {
    write_json_file(debug_dir, "conda_build_v1_error.json", error).await
}

async fn write_json_file<T: Serialize>(
    debug_dir: &Path,
    file_name: &str,
    value: &T,
) -> miette::Result<()> {
    tokio_fs::create_dir_all(debug_dir)
        .await
        .into_diagnostic()
        .context("failed to create debug directory")?;

    let json = serde_json::to_string_pretty(value)
        .into_diagnostic()
        .context("failed to serialize value to JSON")?;

    let path = debug_dir.join(file_name);
    tokio_fs::write(&path, json)
        .await
        .into_diagnostic()
        .context("failed to write JSON to file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_build_types::{
        BackendCapabilities,
        procedures::{
            initialize::InitializeResult,
            log_message::{LogEvent, LogLevel},
            negotiate_capabilities::NegotiateCapabilitiesResult,
        },
    };
    use tokio::io::AsyncWriteExt;

    struct FakeInstantiator;

    #[async_trait::async_trait]
    impl ProtocolInstantiator for FakeInstantiator {
        async fn negotiate_capabilities(
            _params: NegotiateCapabilitiesParams,
        ) -> miette::Result<NegotiateCapabilitiesResult> {
            Ok(NegotiateCapabilitiesResult {
                capabilities: BackendCapabilities::default(),
            })
        }

        async fn initialize(
            &self,
            _params: InitializeParams,
        ) -> miette::Result<(Box<dyn Protocol + Send + Sync + 'static>, InitializeResult)> {
            unimplemented!("not needed for this test")
        }
    }

    #[tokio::test]
    async fn responses_and_log_notifications_share_the_output_channel() {
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let enabled = Arc::new(AtomicBool::new(false));

        let (server_read, mut client_write) = tokio::io::simplex(4096);
        let (client_read, server_write) = tokio::io::simplex(4096);

        let enabled_check = enabled.clone();
        let server = tokio::spawn(
            Server::new(FakeInstantiator)
                .with_log_messages(log_rx, enabled.clone())
                .run_with_io(server_read, server_write),
        );

        // Records sent before negotiation must still be forwarded (the
        // *layer* is what holds records back until activation; the server
        // forwards whatever reaches it).
        client_write
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"negotiateCapabilities\",\
                \"params\":{\"capabilities\":{\"supportsLogMessages\":true}}}\n",
            )
            .await
            .unwrap();

        let mut lines = BufReader::new(client_read).lines();
        let response = lines.next_line().await.unwrap().unwrap();
        assert!(response.contains("\"id\":1"));
        assert!(
            enabled_check.load(Ordering::Acquire),
            "negotiation must flip the activation flag"
        );

        log_tx
            .send(LogMessage::Event(LogEvent {
                level: LogLevel::Warn,
                target: "t".to_string(),
                message: "hello".to_string(),
                fields: Default::default(),
                span_id: None,
            }))
            .unwrap();
        let notification = lines.next_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&notification).unwrap();
        assert_eq!(parsed["method"], "log/message");
        assert_eq!(parsed["params"]["kind"], "event");
        assert_eq!(parsed["params"]["message"], "hello");
        assert!(parsed.get("id").is_none(), "notifications have no id");

        // Closing the input shuts the server down cleanly. Note: merely
        // dropping the write half would not close the underlying simplex
        // stream; an explicit shutdown is required to produce an EOF.
        client_write.shutdown().await.unwrap();
        server.await.unwrap().unwrap();
    }
}
