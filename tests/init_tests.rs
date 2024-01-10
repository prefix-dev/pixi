mod common;

use crate::common::PixiControl;
use rattler_conda_types::{Channel, ChannelConfig, Version};
use std::str::FromStr;

#[tokio::test]
async fn init_creates_project_manifest() {
    // Run the init command
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // There should be a loadable project manifest in the directory
    let project = pixi.project().unwrap();

    // Default configuration should be present in the file
    assert!(!project.name().is_empty());
    assert_eq!(
        project.name(),
        pixi.project_path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .as_ref(),
        "project name should match the directory name"
    );
    assert_eq!(
        project.version().as_ref().unwrap(),
        &Version::from_str("0.1.0").unwrap()
    );
}

/// Tests that when initializing an empty project with a custom channel it is actually used.
#[tokio::test]
async fn specific_channel() {
    let pixi = PixiControl::new().unwrap();

    // Init with a custom channel
    pixi.init()
        .with_channel("random")
        .with_channel("foobar")
        .await
        .unwrap();

    // Load the project
    let project = pixi.project().unwrap();

    // The only channel should be the "random" channel
    let channels = project.channels();
    assert_eq!(
        channels,
        &[
            Channel::from_str("random", &ChannelConfig::default()).unwrap(),
            Channel::from_str("foobar", &ChannelConfig::default()).unwrap()
        ]
    )
}

/// Tests that when initializing an empty project the default channel `conda-forge` is used.
#[tokio::test]
async fn default_channel() {
    let pixi = PixiControl::new().unwrap();

    // Init a new project
    pixi.init().await.unwrap();

    // Load the project
    let project = pixi.project().unwrap();

    // The only channel should be the "conda-forge" channel
    let channels = project.channels();
    assert_eq!(
        channels,
        &[Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap()]
    )
}
