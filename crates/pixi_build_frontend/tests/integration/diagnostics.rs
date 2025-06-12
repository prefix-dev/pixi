use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use pixi_build_frontend::{BuildFrontend, InProcessBackend, SetupRequest, ToolContext};
use pixi_manifest::{
    BuildBackend, KnownPreviewFeature, Package, PackageBuild, PackageManifest, Preview, Workspace,
    WorkspaceManifest,
    toml::{ExternalWorkspaceProperties, FromTomlStr, TomlManifest},
};
use pixi_spec::BinarySpec;
use pixi_test_utils::format_diagnostic;
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::{
    io::{CopyToBytes, SinkWriter, StreamReader},
    sync::PollSender,
};

fn test_data_dir() -> PathBuf {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap();
    root_dir.join("tests").join("data")
}

fn build_backends_dir() -> PathBuf {
    test_data_dir().join("build-backends")
}

/// Returns the channel to use when using the build backends in the tests
/// directory.
fn build_backends_channel() -> NamedChannelOrUrl {
    let backends_dir = build_backends_dir();
    NamedChannelOrUrl::Path(backends_dir.display().to_string().into())
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

    insta::assert_snapshot!(format_diagnostic(&err));
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

    let snapshot = format_diagnostic(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
}

#[tokio::test]
async fn test_invalid_manifest() {
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::WORKSPACE_MANIFEST);
    tokio::fs::write(&manifest, "[workspace]").await.unwrap();
    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: source_dir.path().to_path_buf(),
            build_tool_override: Default::default(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    let snapshot = format_diagnostic(&err);
    let snapshot = replace_source_dir(
        &snapshot,
        dunce::canonicalize(source_dir.path()).unwrap().as_path(),
    );

    insta::assert_snapshot!(snapshot);
}

fn replace_source_dir(snapshot: &str, source_dir: &Path) -> String {
    snapshot.replace(
        &(source_dir.display().to_string().replace("\\", "/") + std::path::MAIN_SEPARATOR_STR),
        "[SOURCE_DIR]/",
    )
}

#[tokio::test]
async fn test_not_a_package() {
    // Setup a temporary project
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::WORKSPACE_MANIFEST);
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

    let snapshot = format_diagnostic(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
}

/// This test checks the diagnostic message that is emitted if a backend is used
/// that does not contain the expected executable.
#[tokio::test]
async fn missing_backend_executable() {
    // Construct an in memory manifest for a workspace and a package.
    let workspace = WorkspaceManifest {
        workspace: Workspace {
            platforms: [Platform::current()].into(),
            preview: Preview::from_iter([KnownPreviewFeature::PixiBuild]),
            ..Workspace::default()
        },
        ..WorkspaceManifest::default()
    };

    let package = PackageManifest {
        package: Package::new("project".into(), "0.1.0".parse().unwrap()),
        build: PackageBuild::new(
            BuildBackend {
                name: PackageName::new_unchecked("empty-backend"),
                spec: BinarySpec::any(),
            },
            vec![build_backends_channel()],
        ),
        targets: Default::default(),
    };

    // Construct a protocol for the workspace and package.
    let err = pixi_build_frontend::pixi_protocol::ProtocolBuilder::new(
        PathBuf::from("."),
        PathBuf::from(pixi_consts::consts::WORKSPACE_MANIFEST),
        workspace,
        package,
    )
    .finish(ToolContext::default().into(), 0)
    .await
    .unwrap_err();

    // Check that the error message contains the expected text.
    insta::assert_snapshot!(format_diagnostic(&err));
}

#[tokio::test]
async fn test_invalid_backend() {
    // Setup a temporary project
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::WORKSPACE_MANIFEST);

    let toml = r#"
    [workspace]
    platforms = []
    channels = []
    preview = ['pixi-build']

    [package]
    version = "0.1.0"
    name = "project"

    [package.build]
    backend = { name = "ipc", version = "*" }
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

    let (workspace, package, _) = TomlManifest::from_toml_str(toml)
        .unwrap()
        .into_workspace_manifest(ExternalWorkspaceProperties::default(), None)
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

    let snapshot = format_diagnostic(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
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
