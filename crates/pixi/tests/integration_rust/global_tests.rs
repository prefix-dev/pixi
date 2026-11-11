use crate::common::{PixiControl, workspaces_dir};
use crate::setup_tracing;
use pixi_build_backend_passthrough::{BackendEvent, ObservableBackend, PassthroughBackend};
use pixi_build_frontend::BackendOverride;

/// Test that force-reinstall triggers rebuilding the package
#[tokio::test]
async fn test_source_package_with_passthrough_backend_for_global() {
    setup_tracing();

    // Create an observable backend and get the observer
    let (instantiator, mut observer) =
        ObservableBackend::instantiator(PassthroughBackend::instantiator());

    // Create a PixiControl instance with ObservableBackend
    let backend_override = BackendOverride::from_memory(instantiator);
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let root_dir = workspaces_dir()
        .join("source-backends")
        .join("source-package");

    // First install - should trigger conda_build_v1
    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .await
        .unwrap();

    // Verify that conda_build_v1 was called
    let events = observer.events();
    assert!(events.contains(&BackendEvent::CondaBuildV1Called));

    // Second install - should NOT trigger conda_build_v1 (package is cached)
    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .await
        .unwrap();

    // Verify that conda_build_v1 was *NOT* called
    let events = observer.events();
    assert!(!events.contains(&BackendEvent::CondaBuildV1Called));

    // Third install with force-reinstall - should trigger conda_build_v1 again
    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .with_force_reinstall(true)
        .await
        .unwrap();

    // Verify that conda_build_v1 was called again
    let events = observer.events();
    assert!(events.contains(&BackendEvent::CondaBuildV1Called));
}
