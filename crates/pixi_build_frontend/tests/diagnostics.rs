use std::path::Path;

use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};
use pixi_build_frontend::{BuildFrontend, InProcessBackend, SetupRequest};

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
async fn test_source_dir_is_file() {
    let source_file = tempfile::NamedTempFile::new().unwrap();
    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: source_file.path().to_path_buf(),
            build_tool_override: Default::default(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    let snapshot = error_to_snapshot(&err);
    let snapshot = snapshot.replace(&source_file.path().display().to_string(), "[SOURCE_FILE]");
    insta::assert_snapshot!(snapshot);
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
async fn test_missing_backend() {
    // Setup a temporary project
    let source_dir = tempfile::TempDir::new().unwrap();
    let manifest = source_dir
        .path()
        .join(pixi_consts::consts::PROJECT_MANIFEST);
    tokio::fs::write(
        &manifest,
        r#"
        [workspace]
        platforms = []
        channels = []
        preview = ['pixi-build']

        [package]
        name = "project"
        version = "0.1.0"

        [build-system]
        dependencies = []
        build-backend = "non-existing"
        channels = []
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
    tokio::fs::write(
        &manifest,
        r#"
        [workspace]
        platforms = []
        channels = []
        preview = ['pixi-build']

        [package]
        version = "0.1.0"
        name = "project"

        [build-system]
        dependencies = []
        channels = []
        build-backend = "ipc"
        "#,
    )
    .await
    .unwrap();

    let (in_tx, in_rx) = tokio::io::duplex(1024);
    let (out_tx, _out_rx) = tokio::io::duplex(1024);
    let ipc = InProcessBackend {
        rpc_in: Box::new(in_rx),
        rpc_out: Box::new(out_tx),
    };

    // Explicitly drop the sending end of the channel to simulate a closed
    // connection.
    drop(in_tx);

    let err = BuildFrontend::default()
        .setup_protocol(SetupRequest {
            source_dir: source_dir.path().to_path_buf(),
            build_tool_override: ipc.into(),
            build_id: 0,
        })
        .await
        .unwrap_err();

    let snapshot = error_to_snapshot(&err);
    let snapshot = replace_source_dir(&snapshot, source_dir.path());
    insta::assert_snapshot!(snapshot);
}
