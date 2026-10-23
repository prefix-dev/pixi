use crate::common::{PixiControl, workspaces_dir};
use crate::setup_tracing;
use pixi_build_backend_passthrough::{BackendEvent, ObservableBackend, PassthroughBackend};
use pixi_build_frontend::BackendOverride;
use pixi_test_utils::{LocalChannel, MockRepoData, Package};

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

/// Writes a global manifest with one `tool = "*"` environment that uses the
/// given channels, in priority order.
fn write_global_manifest(pixi: &PixiControl, channels: &[&LocalChannel]) {
    let manifests = pixi.global_home().join("manifests");
    fs_err::create_dir_all(&manifests).unwrap();
    let channels = channels
        .iter()
        .map(|channel| format!("\"{}\"", channel.url()))
        .collect::<Vec<_>>()
        .join(", ");
    fs_err::write(
        manifests.join("pixi-global.toml"),
        format!(
            r#"version = 1

[envs.tool]
channels = [{channels}]
dependencies = {{ tool = "*" }}
exposed = {{}}
"#
        ),
    )
    .unwrap();
}

/// The file names in an installed global environment's `conda-meta`.
fn conda_meta_entries(pixi: &PixiControl, env: &str) -> Vec<String> {
    fs_err::read_dir(pixi.global_home().join("envs").join(env).join("conda-meta"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// `tool 2` is in the highest-priority channel, but needs `libfoo >=2`, which
/// only a lower-priority channel has. Strict channel priority hides it, so the
/// solve falls back to `tool 1`. `pixi global update` used to report that as
/// "unchanged"; it has to install `tool 1`, then fail and say why `tool 2`
/// can't be installed.
#[tokio::test]
async fn global_update_fails_when_newer_version_is_blocked() {
    setup_tracing();

    let primary = MockRepoData::default()
        .with_package(
            Package::build("tool", "1")
                .with_dependency("libfoo >=1")
                .with_materialize(true)
                .finish(),
        )
        .with_package(
            Package::build("tool", "2")
                .with_dependency("libfoo >=2")
                .with_materialize(true)
                .finish(),
        )
        .with_package(
            Package::build("libfoo", "1")
                .with_materialize(true)
                .finish(),
        )
        .into_channel()
        .await
        .unwrap();
    let secondary = MockRepoData::default()
        .with_package(
            Package::build("libfoo", "2")
                .with_materialize(true)
                .finish(),
        )
        .into_channel()
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();
    write_global_manifest(&pixi, &[&primary, &secondary]);

    pixi.global_update()
        .await
        .expect_err("a newer `tool` that can't be installed must fail the update");
    assert!(
        conda_meta_entries(&pixi, "tool")
            .iter()
            .any(|name| name.starts_with("tool-1-")),
        "the newest solvable version is still installed"
    );

    // The CLI prints the per-environment reason and returns a summary; ask
    // the global project directly to check the reason itself.
    let err = temp_env::async_with_vars(
        [
            ("PIXI_HOME", Some(pixi.global_home().to_path_buf())),
            ("PIXI_CACHE_DIR", Some(pixi.global_home().to_path_buf())),
        ],
        async {
            let config = pixi_config::Config::with_cli_config(&pixi.config_cli());
            let project = pixi_global::Project::discover_or_create()
                .await
                .unwrap()
                .with_cli_config(config);
            let env_name = "tool".parse().unwrap();
            let (_, blocked_upgrade) = project.update_environment(&env_name).await.unwrap();
            blocked_upgrade
        },
    )
    .await
    .expect("a newer `tool` that can't be installed must be reported");
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("tool 2") && rendered.contains("stays at 1"),
        "the error must name the blocked version: {rendered}"
    );
    assert!(
        rendered.contains("libfoo 2") && rendered.contains("strict channel priority"),
        "the error must carry the solver's reason: {rendered}"
    );
}

/// The same channels, but the newer dependency is in the primary channel too,
/// so `tool 2` is installable and the update just installs it.
#[tokio::test]
async fn global_update_installs_newer_version_when_solvable() {
    setup_tracing();

    let primary = MockRepoData::default()
        .with_package(
            Package::build("tool", "1")
                .with_dependency("libfoo >=1")
                .with_materialize(true)
                .finish(),
        )
        .with_package(
            Package::build("tool", "2")
                .with_dependency("libfoo >=2")
                .with_materialize(true)
                .finish(),
        )
        .with_package(
            Package::build("libfoo", "1")
                .with_materialize(true)
                .finish(),
        )
        .with_package(
            Package::build("libfoo", "2")
                .with_materialize(true)
                .finish(),
        )
        .into_channel()
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();
    write_global_manifest(&pixi, &[&primary]);

    pixi.global_update().await.unwrap();
    let tool_meta = conda_meta_entries(&pixi, "tool");
    assert!(
        tool_meta.iter().any(|name| name.starts_with("tool-2-")),
        "expected tool 2 to be installed, found {tool_meta:?}"
    );
}
