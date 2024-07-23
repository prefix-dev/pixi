mod common;

use std::path::PathBuf;

use insta::assert_debug_snapshot;
use pixi::{HasFeatures, Project};
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use tempfile::TempDir;
use url::Url;

use crate::common::{package_database::PackageDatabase, PixiControl};

#[tokio::test]
async fn add_channel() {
    // Create a local package database with no entries and write it to disk. This
    // ensures that we have a valid channel.
    let package_database = PackageDatabase::default();
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
    let project = pixi.project().unwrap();

    // Our channel should be in the list of channels
    let local_channel =
        NamedChannelOrUrl::Url(Url::from_directory_path(additional_channel_dir.path()).unwrap());
    assert!(project
        .default_environment()
        .channels()
        .contains(&local_channel));
}

#[tokio::test]
async fn parse_project() {
    fn dependency_names(project: &Project, platform: Platform) -> Vec<String> {
        project
            .default_environment()
            .dependencies(None, Some(platform))
            .iter()
            .map(|dep| dep.0.as_normalized().to_string())
            .collect()
    }

    let pixi_toml = include_str!("./pixi_tomls/many_targets.toml");
    let project = Project::from_str(&PathBuf::from("./many/pixi.toml"), pixi_toml).unwrap();
    assert_debug_snapshot!(dependency_names(&project, Platform::Linux64));
    assert_debug_snapshot!(dependency_names(&project, Platform::OsxArm64));
    assert_debug_snapshot!(dependency_names(&project, Platform::Win64));
}

#[tokio::test]
async fn parse_valid_schema_projects() {
    // Test all files in the schema/examples/valid directory
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/examples/valid");
    for entry in std::fs::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let pixi_toml = std::fs::read_to_string(&path).unwrap();
            let _project = Project::from_str(&PathBuf::from("pixi.toml"), &pixi_toml).unwrap();
        }
    }
}
