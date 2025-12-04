use indexmap::IndexMap;
use insta::assert_snapshot;
use pixi_cli::upgrade::{Args, parse_specs_for_platform};
use pixi_core::Workspace;
use rattler_conda_types::Platform;
use tempfile::TempDir;
use url::Url;

use crate::common::LockFileExt;
use crate::common::PixiControl;
use crate::common::pypi_index::{Database as PyPIDatabase, PyPIPackage};
use crate::setup_tracing;
use pixi_test_utils::{MockRepoData, Package};

#[tokio::test]
async fn pypi_dependency_index_preserved_on_upgrade() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with python
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    // Create local PyPI index with click
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("click", "8.2.0"))
        .into_simple_index()
        .expect("failed to create local simple index");
    let pypi_index_url = pypi_index.index_url();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel_url}"]
        platforms = ["{platform}"]

        [pypi-dependencies]
        click = {{ version = "==8.2.0", index = "{pypi_index_url}" }}

        [dependencies]
        python = "==3.12.0""#,
        channel_url = channel.url(),
        platform = platform,
        pypi_index_url = pypi_index_url,
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

    // Redact platform-specific and path-specific information for consistent snapshots
    let content = pixi.manifest_contents().unwrap_or_default();
    let redacted_content = content
        .replace(&Platform::current().to_string(), "[PLATFORM]")
        .replace(&channel.url().to_string(), "[CHANNEL_URL]")
        .replace(&pypi_index_url.to_string(), "[PYPI_INDEX_URL]");
    assert_snapshot!(redacted_content, @r###"
        [workspace]
        channels = ["[CHANNEL_URL]"]
        platforms = ["[PLATFORM]"]

        [pypi-dependencies]
        click = { version = ">=8.3.1, <9", index = "[PYPI_INDEX_URL]" }

        [dependencies]
        python = ">=3.12.0,<3.13"
    "###);
}

#[tokio::test]
async fn upgrade_command_updates_platform_specific_version() {
    setup_tracing();

    let platform = Platform::current();
    let mut package_database = MockRepoData::default();
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

#[tokio::test]
async fn upgrade_command_updates_all_platform_specific_targets() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
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

/// Test that `pixi upgrade` uses the per-package `index` URL when fetching
/// available versions, not the default index-url.
///
/// Setup:
/// - Default index has `foo-1.0.0`
/// - Custom index has `foo-2.0.0`
/// - Manifest specifies `foo = { version = "==1.0.0", index = "<custom>" }`
///
/// Expected: After upgrade, `foo` should be upgraded to 2.0.0 (from custom index)
#[tokio::test]
async fn pypi_dependency_upgrade_uses_custom_index() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Create "default" index with foo 1.0.0 - this should NOT be used for upgrade
    let default_index = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .unwrap();

    // Create "custom" index with foo 1.0.0 AND 2.0.0 - this SHOULD be used for upgrade
    let custom_index = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .with(PyPIPackage::new("foo", "2.0.0"))
        .into_simple_index()
        .unwrap();

    // Create manifest with foo pinned to 1.0.0, using custom index
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-upgrade-custom-index"
        platforms = ["{platform}"]
        channels = ["{channel}"]

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        foo = {{ version = "==1.0.0", index = "{custom_index}" }}

        [pypi-options]
        index-url = "{default_index}"
        "#,
        platform = platform,
        channel = channel.url(),
        default_index = default_index.index_url(),
        custom_index = custom_index.index_url(),
    ))
    .unwrap();

    // First, create the initial lock file
    let _lock_file = pixi.update_lock_file().await.unwrap();

    // Now run upgrade
    let mut args = Args::default();
    args.workspace_config.manifest_path = Some(pixi.manifest_path());
    args.no_install_config.no_install = true;
    args.specs.packages = Some(vec!["foo".to_string()]);

    pixi_cli::upgrade::execute(args).await.unwrap();

    // Load the lock file and verify foo was upgraded to 2.0.0 from the custom index
    let lock_file = pixi.lock_file().await.unwrap();
    let version = lock_file.get_pypi_package_version("default", platform, "foo");

    assert_eq!(
        version,
        Some("2.0.0".into()),
        "foo should be upgraded to 2.0.0 from custom index, not remain at or downgrade to 1.0.0 from default index"
    );

    // Also verify the index is still preserved in the manifest
    let content = pixi.manifest_contents().unwrap_or_default();
    assert!(
        content.contains(&custom_index.index_url().to_string()),
        "custom index URL should be preserved in manifest"
    );
}
