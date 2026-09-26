use std::collections::BTreeSet;
use std::path::PathBuf;
use std::str::FromStr;

use crate::common::PixiControl;
use crate::setup_tracing;
use insta::assert_debug_snapshot;
use pixi_config::Config;
use pixi_core::Workspace;
use pixi_manifest::FeaturesExt;
use pixi_test_utils::MockRepoData;
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use tempfile::TempDir;
use url::Url;

#[tokio::test]
async fn add_remove_channel() {
    setup_tracing();

    // Create a local package database with no entries and write it to disk. This
    // ensures that we have a valid channel.
    let package_database = MockRepoData::default();
    let initial_channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(initial_channel_dir.path())
        .await
        .unwrap();

    // Run the init command
    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(initial_channel_dir.path())
        .await
        .unwrap();

    // Create and add another local package directory
    let additional_channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(additional_channel_dir.path())
        .await
        .unwrap();
    pixi.project_channel_add()
        .with_local_channel(additional_channel_dir.path())
        .await
        .unwrap();

    // There should be a loadable project manifest in the directory
    let project = pixi.workspace().unwrap();

    // Our channel should be in the list of channels
    let local_channel =
        NamedChannelOrUrl::Url(Url::from_file_path(additional_channel_dir.as_ref()).unwrap());
    let channels = project.default_environment().channels();
    assert!(channels.len() == 2);
    assert!(channels.last().unwrap() == &&local_channel);
    assert!(channels.contains(&local_channel));

    // now add the same channel, with priority 2
    pixi.project_channel_add()
        .with_local_channel(additional_channel_dir.path())
        .with_priority(Some(2i32))
        .await
        .unwrap();

    // Load again
    let project = pixi.workspace().unwrap();
    let channels = project.default_environment().channels();
    // still present
    assert!(channels.contains(&local_channel));
    // didn't duplicate
    assert!(channels.len() == 2);
    // priority applied
    assert!(channels.first().unwrap() == &&local_channel);

    // now remove it
    pixi.project_channel_remove()
        .with_local_channel(additional_channel_dir.path())
        .await
        .unwrap();

    // Load again
    let project = pixi.workspace().unwrap();
    let channels = project.default_environment().channels();

    // Channel has been removed
    assert!(channels.len() == 1);
    assert!(!channels.contains(&local_channel));
}

#[tokio::test]
async fn parse_project() {
    setup_tracing();

    fn dependency_names(project: &Workspace, platform: Platform) -> Vec<String> {
        let pp = pixi_manifest::PixiPlatform::from_subdir(platform);
        project
            .default_environment()
            .combined_dependencies(Some(&pp))
            .iter()
            .map(|dep| dep.0.as_normalized().to_string())
            .collect()
    }

    let pixi_toml = include_str!("../../../../tests/data/pixi_tomls/many_targets.toml");
    let project = Workspace::from_str(&PathBuf::from("./many/pixi.toml"), pixi_toml).unwrap();
    assert_debug_snapshot!(dependency_names(&project, Platform::Linux64));
    assert_debug_snapshot!(dependency_names(&project, Platform::OsxArm64));
    assert_debug_snapshot!(dependency_names(&project, Platform::Win64));
}

#[tokio::test]
async fn parse_valid_schema_projects() {
    setup_tracing();

    // Test all files in the schema/examples/valid directory
    let schema_dir = PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("schema/examples/valid");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let pixi_toml = fs_err::read_to_string(&path).unwrap();
            // Fake manifest path to be CARGO_WORKSPACE_DIR/pixi.toml
            // so the test is able to find a valid LICENSE file.
            let manifest_path = PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("pixi.toml");
            if let Err(e) = Workspace::from_str(&manifest_path, &pixi_toml) {
                panic!("Error parsing {}: {}", path.display(), e);
            }
        }
    }
}

#[test]
fn parse_valid_docs_manifests() {
    setup_tracing();

    // Test all files in the docs/source_files/pixi_tomls directory
    let schema_dir =
        PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("docs/source_files/pixi_tomls");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let pixi_toml = fs_err::read_to_string(&path).unwrap();
            // Fake manifest path to be CARGO_WORKSPACE_DIR/pixi.toml
            // so the test is able to find a valid LICENSE file.
            let manifest_path = PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("pixi.toml");
            if let Err(e) = Workspace::from_str(&manifest_path, &pixi_toml) {
                panic!("Error parsing {}: {}", path.display(), e);
            }
        }
    }
}
#[test]
fn parse_valid_docs_pyproject_manifests() {
    setup_tracing();

    // Test all files in the docs/source_files/pyproject_tomls directory
    let schema_dir =
        PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("docs/source_files/pyproject_tomls");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let pyproject_toml = fs_err::read_to_string(&path).unwrap();
            let _project =
                Workspace::from_str(&PathBuf::from("pyproject.toml"), &pyproject_toml).unwrap();
        }
    }
}

#[test]
fn parse_valid_docs_configs() {
    setup_tracing();

    // Test all files in the docs/source_files/pixi_config_tomls directory
    let schema_dir =
        PathBuf::from(env!("CARGO_WORKSPACE_DIR")).join("docs/source_files/pixi_config_tomls");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let toml = fs_err::read_to_string(&path).unwrap();
            let (_config, unused_keys) = Config::from_toml(&toml, None).unwrap();
            assert_eq!(
                unused_keys,
                BTreeSet::<String>::new(),
                "{}",
                format_args!("Unused keys in {:?}", path)
            );
        }
    }
}

#[tokio::test]
async fn test_workspace_requires_python_cli() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    let pyproject_path = pixi.workspace_path().join("pyproject.toml");
    fs_err::write(
        &pyproject_path,
        r#"[project]
name = "test-pyproject"
version = "0.1.0"
requires-python = ">=3.11"

[tool.pixi.workspace]
channels = []
platforms = ["linux-64"]
"#,
    )
    .unwrap();

    let workspace = pixi.workspace().unwrap();
    assert_eq!(
        workspace.workspace.value.workspace.requires_python,
        Some(pixi_manifest::pep440_rs::VersionSpecifiers::from_str(">=3.11").unwrap())
    );

    // Run set command
    let set_args = pixi_cli::workspace::requires_python::set::Args {
        spec: ">=3.12".to_string(),
    };
    pixi_cli::workspace::requires_python::set::execute(workspace, set_args)
        .await
        .unwrap();

    // Verify file content updated
    let content = fs_err::read_to_string(&pyproject_path).unwrap();
    assert!(content.contains("requires-python = \">=3.12\""));

    // Reload workspace and verify
    let workspace = pixi.workspace().unwrap();
    assert_eq!(
        workspace.workspace.value.workspace.requires_python,
        Some(pixi_manifest::pep440_rs::VersionSpecifiers::from_str(">=3.12").unwrap())
    );

    // Run get command
    pixi_cli::workspace::requires_python::get::execute(workspace)
        .await
        .unwrap();

    let workspace = pixi.workspace().unwrap();

    // Run unset command
    pixi_cli::workspace::requires_python::unset::execute(workspace)
        .await
        .unwrap();

    let content = fs_err::read_to_string(&pyproject_path).unwrap();
    assert!(!content.contains("requires-python"));

    let workspace = pixi.workspace().unwrap();
    assert_eq!(workspace.workspace.value.workspace.requires_python, None);

    // Test on pixi.toml fails
    let pixi2 = PixiControl::new().unwrap();
    pixi2.init().await.unwrap();
    let ws2 = pixi2.workspace().unwrap();
    let set_args = pixi_cli::workspace::requires_python::set::Args {
        spec: ">=3.12".to_string(),
    };
    assert!(
        pixi_cli::workspace::requires_python::set::execute(ws2, set_args)
            .await
            .is_err()
    );
}
