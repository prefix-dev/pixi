mod common;

use crate::{common::package_database::PackageDatabase, common::PixiControl};
use pixi::cli::run;
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
async fn description() {
    // Get a pixi instance
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let _command = "project description set \"Hello world\"";

    // // Set the description
    // let result = pixi
    //     .run(run::Args {
    //         task: command.split(" ").map(|s| s.to_string()).collect(),
    //         ..Default::default()
    //     })
    //     .await
    //     .unwrap();

    // println!("{:?}", result);

    // assert_eq!(result.exit_code, 0);
    // assert_eq!(result.stdout.trim(), "Python 3.11.0");
    // assert!(result.stderr.is_empty());

    // let project = pixi.project().unwrap();
}
