mod common;

use crate::{common::package_database::PackageDatabase, common::PixiControl};
use pixi::cli::project::description;
use rattler_conda_types::{Channel, ChannelConfig};
use tempfile::TempDir;
use url::Url;

#[tokio::test]
async fn add_channel() {
    // Create a local package database with no entries and write it to disk. This ensures that we
    // have a valid channel.
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
    let local_channel = Channel::from_str(
        Url::from_directory_path(additional_channel_dir.path())
            .unwrap()
            .to_string(),
        &ChannelConfig::default(),
    )
    .unwrap();
    assert!(project.channels().contains(&local_channel));
}

#[tokio::test]
async fn description_set() {
    // Get a pixi instance
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let new_description = "Hello description 1234567890!";

    // Set the description
    description::execute(description::Args {
        command: description::Command::Set(description::set::Args {
            description: new_description.to_string(),
        }),
        manifest_path: Some(pixi.project().unwrap().manifest_path()),
    })
    .await
    .unwrap();

    // Load the project
    let project = pixi.project().unwrap();

    // Check that the description is set
    assert_eq!(project.description().as_ref().unwrap(), new_description);
}
