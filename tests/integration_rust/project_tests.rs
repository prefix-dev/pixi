use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::common::{package_database::PackageDatabase, PixiControl};
use insta::assert_debug_snapshot;
use pixi::Project;
use pixi_config::Config;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use tempfile::TempDir;
use url::Url;

#[tokio::test]
async fn add_remove_channel() {
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
    let project = Project::from_path(&pixi.manifest_path()).unwrap();

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
    let project = Project::from_path(&pixi.manifest_path()).unwrap();
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
    let project = Project::from_path(&pixi.manifest_path()).unwrap();
    let channels = project.default_environment().channels();

    // Channel has been removed
    assert!(channels.len() == 1);
    assert!(!channels.contains(&local_channel));
}

#[tokio::test]
async fn parse_project() {
    fn dependency_names(project: &Project, platform: Platform) -> Vec<String> {
        project
            .default_environment()
            .combined_dependencies(Some(platform))
            .iter()
            .map(|dep| dep.0.as_normalized().to_string())
            .collect()
    }

    let pixi_toml = include_str!("../data/pixi_tomls/many_targets.toml");
    let project = Project::from_str(&PathBuf::from("./many/pixi.toml"), pixi_toml).unwrap();
    assert_debug_snapshot!(dependency_names(&project, Platform::Linux64));
    assert_debug_snapshot!(dependency_names(&project, Platform::OsxArm64));
    assert_debug_snapshot!(dependency_names(&project, Platform::Win64));
}

#[tokio::test]
async fn parse_valid_schema_projects() {
    // Test all files in the schema/examples/valid directory
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/examples/valid");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let pixi_toml = fs_err::read_to_string(&path).unwrap();
            let _project = Project::from_str(&PathBuf::from("pixi.toml"), &pixi_toml).unwrap();
        }
    }
}

#[test]
fn parse_valid_docs_manifests() {
    // Test all files in the docs/source_files/pixi_tomls directory
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/source_files/pixi_tomls");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let pixi_toml = fs_err::read_to_string(&path).unwrap();
            let _project = Project::from_str(&PathBuf::from("pixi.toml"), &pixi_toml).unwrap();
        }
    }
}

#[test]
fn parse_valid_docs_configs() {
    // Test all files in the docs/source_files/pixi_config_tomls directory
    let schema_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/source_files/pixi_config_tomls");
    for entry in fs_err::read_dir(schema_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|ext| ext == "toml").unwrap_or(false) {
            let toml = fs_err::read_to_string(&path).unwrap();
            let (_config, unused_keys) = Config::from_toml(&toml).unwrap();
            assert_eq!(
                unused_keys,
                BTreeSet::<String>::new(),
                "{}",
                format_args!("Unused keys in {:?}", path)
            );
        }
    }
}
