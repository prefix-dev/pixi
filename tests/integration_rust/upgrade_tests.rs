use indexmap::IndexMap;
use insta::assert_snapshot;
use pixi_cli::upgrade::{Args, parse_specs};
use pixi_core::Workspace;
use rattler_conda_types::Platform;

use crate::common::PixiControl;
use crate::setup_tracing;

// This test requires network connection and takes around 40s to
// complete on my machine.
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
#[tokio::test]
async fn pypi_dependency_index_preserved_on_upgrade() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["{platform}"]
        exclude-newer = "2025-05-19"

        [pypi-dependencies]
        click = {{ version = "==8.2.0", index = "https://pypi.tuna.tsinghua.edu.cn/simple" }}

        [dependencies]
        python = "==3.13.3""#,
        platform = Platform::current()
    ))
    .unwrap();

    let mut args = Args::default();
    args.workspace_config.manifest_path = Some(pixi.manifest_path());

    let workspace = Workspace::from_path(&pixi.manifest_path()).unwrap();

    let workspace_value = workspace.workspace.value.clone();
    let feature = workspace_value.feature(&args.specs.feature).unwrap();

    let mut workspace = workspace.modify().unwrap();

    let (match_specs, pypi_deps) = parse_specs(feature, &args, &workspace).unwrap();

    let _ = workspace
        .update_dependencies(
            match_specs,
            pypi_deps,
            IndexMap::default(),
            args.no_install_config.no_install,
            &args.lock_file_update_config.lock_file_usage().unwrap(),
            &args.specs.feature,
            &[],
            true,
            args.dry_run,
        )
        .await
        .unwrap();

    workspace.save().await.unwrap();

    // Redact platform-specific information for consistent snapshots across environments
    let content = pixi.manifest_contents().unwrap_or_default();
    let redacted_content = content.replace(&Platform::current().to_string(), "[PLATFORM]");
    assert_snapshot!(redacted_content);
}
