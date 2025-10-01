use indexmap::IndexMap;
use insta::assert_snapshot;
use pixi_cli::upgrade::{Args, parse_specs_for_platform};
use pixi_core::Workspace;
use rattler_conda_types::Platform;
use tempfile::TempDir;
use url::Url;

use crate::common::PixiControl;
use crate::common::package_database::{Package, PackageDatabase};
use crate::setup_tracing;

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
    args.no_install_config.no_install = true;

    let workspace = Workspace::from_path(&pixi.manifest_path()).unwrap();

    let workspace_value = workspace.workspace.value.clone();
    let feature = workspace_value.default_feature();
    let mut workspace = workspace.modify().unwrap();

    let (match_specs, pypi_deps) =
        parse_specs_for_platform(feature, &args, &workspace, None).unwrap();

    let _ = workspace
        .update_dependencies(
            match_specs,
            pypi_deps,
            IndexMap::default(),
            args.no_install_config.no_install,
            &args.lock_file_update_config.lock_file_usage().unwrap(),
            &feature.name,
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

#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
#[tokio::test]
async fn upgrade_command_updates_platform_specific_version() {
    setup_tracing();

    let platform = Platform::current();
    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("python", "3.12.0").finish());
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();
    let channel = Url::from_file_path(channel_dir.path()).unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel}"]
        platforms = ["{platform}"]
        exclude-newer = "2025-05-19"

        [dependencies]

        [target.{platform}.dependencies]
        python = "==3.12"
        "#,
        platform = platform,
        channel = channel,
    ))
    .unwrap();

    let mut args = Args::default();
    args.workspace_config.manifest_path = Some(pixi.manifest_path());
    args.no_install_config.no_install = true;

    pixi_cli::upgrade::execute(args).await.unwrap();

    let content = pixi.manifest_contents().unwrap_or_default();

    assert!(
        !content.contains("python = \"==3.12\""),
        "python pin should be removed from manifest"
    );
    assert!(
        content.contains("python = \">=3."),
        "python version should be upgraded to a >=3.x,<3.(x+1) range"
    );
}

#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
#[tokio::test]
async fn upgrade_command_updates_all_platform_specific_targets() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("python", "3.12.0").finish());
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();
    let channel = Url::from_file_path(channel_dir.path()).unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel}"]
        platforms = ["linux-64", "win-64"]
        exclude-newer = "2025-05-19"

        [target.linux-64.dependencies]
        python = "==3.12"

        [target.win-64.dependencies]
        python = "==3.12"
        "#,
        channel = channel,
    ))
    .unwrap();

    let mut args = Args::default();
    args.workspace_config.manifest_path = Some(pixi.manifest_path());
    args.no_install_config.no_install = true;

    pixi_cli::upgrade::execute(args).await.unwrap();

    let content = pixi.manifest_contents().unwrap_or_default();

    assert!(
        !content.contains("==3.12"),
        "python pins should be removed from all platform-specific targets"
    );

    let upgraded_occurrences = content.matches("python = \">=3.").count();
    assert!(
        upgraded_occurrences == 2,
        "expected at least two upgraded python entries, found {upgraded_occurrences}:\n{content}"
    );
    assert!(content.contains("[target.linux-64.dependencies]"));
    assert!(content.contains("[target.win-64.dependencies]"));
}
