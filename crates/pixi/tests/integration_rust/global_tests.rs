use crate::common::{PixiControl, workspaces_dir};
use crate::setup_tracing;
use pixi_build_backend_passthrough::{BackendEvent, ObservablePassthroughBackend};
use pixi_build_frontend::BackendOverride;
use std::time::Duration;

/// Test that force-reinstall triggers rebuilding the package
#[tokio::test]
async fn test_source_package_with_passthrough_backend_for_global() {
    setup_tracing();

    // Create a channel to receive backend events
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BackendEvent>();

    // Create a PixiControl instance with ObservablePassthroughBackend
    let backend_override =
        BackendOverride::from_memory(ObservablePassthroughBackend::instantiator(tx));
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let root_dir = workspaces_dir().join("source-backends").join("package-e");

    // First install - should trigger conda_build_v1
    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .await
        .unwrap();

    // Verify that conda_build_v1 was called (with timeout)
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1);

    loop {
        tokio::select! {
            Some(event) = rx.recv() => events.push(event),
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(events.contains(&BackendEvent::CondaBuildV1Called));

    // Second install - should NOT trigger conda_build_v1 (package is cached)
    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .await
        .unwrap();

    // Verify that conda_build_v1 was *NOT* called (with timeout)
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1);

    loop {
        tokio::select! {
            Some(event) = rx.recv() => events.push(event),
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(!events.contains(&BackendEvent::CondaBuildV1Called));

    // Third install with force-reinstall - should trigger conda_build_v1 again
    pixi.global_install()
        .with_path(root_dir.to_string_lossy())
        .with_force_reinstall(true)
        .await
        .unwrap();

    // Verify that conda_build_v1 was called again (with timeout)
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1);

    loop {
        tokio::select! {
            Some(event) = rx.recv() => events.push(event),
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    assert!(events.contains(&BackendEvent::CondaBuildV1Called));
}
