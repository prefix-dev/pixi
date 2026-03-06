use std::{net::SocketAddr, path::Path, sync::Arc};

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
        negotiate_capabilities::NegotiateCapabilitiesParams,
    },
};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

use crate::consts::DEBUG_OUTPUT_DIR;
use crate::protocol::{Protocol, ProtocolInstantiator};

/// A JSONRPC server that can be used to communicate with a client.
pub struct Server<T: ProtocolInstantiator> {
    instatiator: T,
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
        Self { instatiator }
    }

    /// Run the server, communicating over stdin/stdout.
    pub async fn run(self) -> miette::Result<()> {
        let io = self.setup_io();
        jsonrpc_stdio_server::ServerBuilder::new(io).build().await;
        Ok(())
    }

    /// Run the server, communicating over HTTP.
    pub fn run_over_http(self, port: u16) -> miette::Result<()> {
        let io = self.setup_io();
        jsonrpc_http_server::ServerBuilder::new(io)
            .start_http(&SocketAddr::from(([127, 0, 0, 1], port)))
            .into_diagnostic()?
            .wait();
        Ok(())
    }

    /// Setup the IO inner handler.
    fn setup_io(self) -> IoHandler {
        // Construct a server
        let mut io = IoHandler::new();
        io.add_method(
            procedures::negotiate_capabilities::METHOD_NAME,
            move |params: Params| async move {
                let params: NegotiateCapabilitiesParams = params.parse()?;
                let result = T::negotiate_capabilities(params)
                    .await
                    .map_err(convert_error)?;
                Ok(to_value(result).expect("failed to convert to json"))
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
