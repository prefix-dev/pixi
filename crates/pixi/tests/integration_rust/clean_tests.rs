use crate::common::isolated_config_source;
use crate::setup_tracing;
use pixi_cli::clean::{self, Args};
use pixi_cli::cli_config::{ScriptWorkspaceConfig, WorkspaceConfig};
use pixi_core::WorkspaceLocator;
use tempfile::tempdir;

#[tokio::test]
async fn test_clean_script_removes_cache() {
    setup_tracing();

    let temp = tempdir().unwrap();
    let script_path = temp.path().join("test_script.py");
    fs_err::write(
        &script_path,
        r#"# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
print("hello")
"#,
    )
    .unwrap();

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(pixi_core::workspace::DiscoveryStart::Script(
            script_path.clone(),
        ))
        .locate()
        .unwrap();

    let script_pixi_dir = workspace.pixi_dir();
    fs_err::create_dir_all(&script_pixi_dir).unwrap();
    fs_err::write(script_pixi_dir.join("dummy_artifact.txt"), "cached").unwrap();
    assert!(script_pixi_dir.exists());

    clean::execute(Args {
        config_source: isolated_config_source(),
        workspace_config: ScriptWorkspaceConfig {
            workspace_config: WorkspaceConfig::default(),
            script: Some(script_path),
        },
        command: None,
        environment: None,
        activation_cache: false,
        build: false,
        workspaces_registry: false,
    })
    .await
    .unwrap();

    assert!(
        !script_pixi_dir.exists(),
        "script pixi_dir {:?} should have been removed by clean --script",
        script_pixi_dir
    );
}

#[tokio::test]
async fn test_clean_conda_script_removes_cache() {
    setup_tracing();

    let temp = tempdir().unwrap();
    let script_path = temp.path().join("test_conda_script.sh");
    fs_err::write(
        &script_path,
        r#"# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "python ${SCRIPT}"
# [dependencies]
# python = "*"
# /// end-conda-script
echo "hello"
"#,
    )
    .unwrap();

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(pixi_core::workspace::DiscoveryStart::Script(
            script_path.clone(),
        ))
        .locate()
        .unwrap();

    let script_pixi_dir = workspace.pixi_dir();
    fs_err::create_dir_all(&script_pixi_dir).unwrap();
    fs_err::write(script_pixi_dir.join("build_artifact.bin"), "data").unwrap();
    assert!(script_pixi_dir.exists());

    clean::execute(Args {
        config_source: isolated_config_source(),
        workspace_config: ScriptWorkspaceConfig {
            workspace_config: WorkspaceConfig::default(),
            script: Some(script_path),
        },
        command: None,
        environment: None,
        activation_cache: false,
        build: false,
        workspaces_registry: false,
    })
    .await
    .unwrap();

    assert!(
        !script_pixi_dir.exists(),
        "conda script pixi_dir {:?} should have been removed by clean --script",
        script_pixi_dir
    );
}
