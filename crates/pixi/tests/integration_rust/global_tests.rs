use crate::common::{PixiControl, workspaces_dir};
use crate::setup_tracing;
use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::BackendOverride;
use std::io::Read;

/// Test that force-reinstall triggers rebuilding the package
#[tokio::test]
async fn test_source_package_with_passthrough_backend_for_global() {
    setup_tracing();

    // Create a PixiControl instance with PassthroughBackend
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let root_dir = workspaces_dir().join("source-backends").join("package-e");

    // Capture stderr
    let mut stderr = gag::BufferRedirect::stderr().unwrap();

    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .await
        .unwrap();

    // Read captured stderr to validate that we started to build the package
    let mut captured_stderr = String::new();
    stderr.read_to_string(&mut captured_stderr).unwrap();

    drop(stderr);

    // Validate that expected content was printed
    assert!(captured_stderr.contains("PassthroughBackend: Starting conda_build_v1"));

    let mut stderr2 = gag::BufferRedirect::stderr().unwrap();
    captured_stderr.clear();

    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .await
        .unwrap();

    stderr2.read_to_string(&mut captured_stderr).unwrap();
    drop(stderr2);

    // On second install, we should already reuse the built package
    assert!(!captured_stderr.contains("PassthroughBackend: Starting conda_build_v1"));

    // Third install with force reinstall should trigger a source rebuild
    let mut stderr = gag::BufferRedirect::stderr().unwrap();
    captured_stderr.clear();

    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .with_force_reinstall(true)
        .await
        .unwrap();

    stderr.read_to_string(&mut captured_stderr).unwrap();

    drop(stderr);

    assert!(captured_stderr.contains("PassthroughBackend: Starting conda_build_v1"));
}
