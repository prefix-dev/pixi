use std::path::Path;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use jsonrpsee::types::Request;
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};
use pixi_build_frontend::{BuildFrontend, InProcessBackend, SetupRequest};
use pixi_build_types::procedures::initialize::InitializeParams;
use pixi_manifest::toml::{ExternalWorkspaceProperties, TomlManifest};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::{
    io::{CopyToBytes, SinkWriter, StreamReader},
    sync::PollSender,
};

fn error_to_snapshot(diag: &impl Diagnostic) -> String {
    let mut report_str = String::new();
    GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .without_syntax_highlighting()
        .with_width(160)
        .render_report(&mut report_str, diag)
        .unwrap();
    report_str
}

#[tokio::test]
async fn test_non_existing_discovery() {
    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: "non/existing/path".into(),
            build_tool_override: Default::default(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    insta::assert_snapshot!(error_to_snapshot(&err));
}

#[tokio::test]
async fn test_source_dir_is_empty() {
    let source_dir = tempfile::TempDir::new().unwrap();
    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: source_dir.path().to_path_buf(),
            build_tool_override: Default::default(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    let snapshot = error_to_snapshot(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
}

#[tokio::test]
async fn test_invalid_manifest() {
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::PROJECT_MANIFEST);
    tokio::fs::write(&manifest, "[workspace]").await.unwrap();
    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: source_dir.path().to_path_buf(),
            build_tool_override: Default::default(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    let snapshot = error_to_snapshot(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());

    insta::assert_snapshot!(snapshot);
}

fn replace_source_dir(snapshot: &str, source_dir: &Path) -> String {
    snapshot.replace(
        &(source_dir.display().to_string() + std::path::MAIN_SEPARATOR_STR),
        "[SOURCE_DIR]/",
    )
}

#[tokio::test]
async fn test_not_a_package() {
    // Setup a temporary project
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::PROJECT_MANIFEST);
    tokio::fs::write(
        &manifest,
        r#"
        [workspace]
        name = "some-workspace"
        platforms = []
        channels = []
        preview = ['pixi-build']
        "#,
    )
    .await
    .unwrap();

    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: source_dir.path().to_path_buf(),
            build_tool_override: Default::default(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    let snapshot = error_to_snapshot(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
}

#[tokio::test]
async fn test_invalid_backend() {
    // Setup a temporary project
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::PROJECT_MANIFEST);

    let toml = r#"
    [workspace]
    platforms = []
    channels = []
    preview = ['pixi-build']

    [package]
    version = "0.1.0"
    name = "project"

    [build-system]
    build-backend = { name = "ipc", version = "*" }
    "#;

    let (frontend_tx, backend_rx) = pipe();
    let (backend_tx, frontend_rx) = pipe();
    let ipc = InProcessBackend {
        rpc_in: Box::new(frontend_rx),
        rpc_out: Box::new(frontend_tx),
    };

    // Explicitly drop the sending end of the channel to simulate a closed
    // connection.
    drop(backend_rx);
    drop(backend_tx);

    let (workspace, package) = TomlManifest::from_toml_str(toml)
        .unwrap()
        .into_manifests(ExternalWorkspaceProperties::default())
        .unwrap();
    let err = pixi_build_frontend::pixi_protocol::ProtocolBuilder::new(
        source_dir.path().to_path_buf(),
        manifest.to_path_buf(),
        workspace,
        package.unwrap(),
    )
    .finish_with_ipc(ipc, 0)
    .await
    .unwrap_err();

    let snapshot = error_to_snapshot(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
}

#[tokio::test]
#[ignore]
async fn test_backend_configuration() {
    let toml = r#"
    [workspace]
    platforms = []
    channels = []
    preview = ['pixi-build']

    [package]
    version = "0.1.0"
    name = "project"

    [build-system]
    build-backend = { name = "ipc", version = "*" }

    [build-backend.ipc]
    hello = "world"
    "#;

    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::PROJECT_MANIFEST);

    let (frontend_tx, backend_rx) = pipe();
    let (backend_tx, frontend_rx) = pipe();
    let ipc = InProcessBackend {
        rpc_in: Box::new(frontend_rx),
        rpc_out: Box::new(frontend_tx),
    };

    let protocol_setup = tokio::spawn(async move {
        let (workspace, package) = TomlManifest::from_toml_str(toml)
            .unwrap()
            .into_manifests(ExternalWorkspaceProperties::default())
            .unwrap();
        pixi_build_frontend::pixi_protocol::ProtocolBuilder::new(
            source_dir.path().to_path_buf(),
            manifest.to_path_buf(),
            workspace,
            package.unwrap(),
        )
        .finish_with_ipc(ipc, 0)
        .await
        .expect_err("the test never sends a response to the initialize request");
    });

    let read_initialize_message = async move {
        let initialize_line = BufReader::new(backend_rx)
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        let request: Request = serde_json::from_str(&initialize_line).unwrap();
        let init_params: InitializeParams = request.params().parse().unwrap();
        drop(backend_tx); // Simulates the backend closing the connection.
        init_params
    };

    let (_, init_params) = tokio::join!(protocol_setup, read_initialize_message);

    insta::assert_snapshot!(serde_json::to_string_pretty(&init_params.configuration).unwrap());
}

/// Creates a pipe that connects an async write instance to an async read
/// instance.
pub fn pipe() -> (
    impl AsyncWrite + Unpin + Send,
    impl AsyncRead + Unpin + Send,
) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(1);

    // Convert the sender into an async write instance
    let sink =
        PollSender::new(tx).sink_map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe));
    let writer = SinkWriter::new(CopyToBytes::new(sink));

    // Convert the receiver into an async read instance
    let stream = ReceiverStream::new(rx).map(Ok::<_, std::io::Error>);
    let reader = StreamReader::new(stream);

    (writer, reader)
}
