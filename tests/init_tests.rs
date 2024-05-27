mod common;

use crate::common::PixiControl;
use pixi::{util::default_channel_config, HasFeatures};
use rattler_conda_types::{Channel, Version};
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
        &pixi.project_path().file_stem().unwrap().to_string_lossy(),
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
    let channels = Vec::from_iter(project.default_environment().channels());
    assert_eq!(
        channels,
        [
            &Channel::from_str("random", &default_channel_config()).unwrap(),
            &Channel::from_str("foobar", &default_channel_config()).unwrap()
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
    let channels = Vec::from_iter(project.default_environment().channels());
    assert_eq!(
        channels,
        [&Channel::from_str("conda-forge", &default_channel_config()).unwrap()]
    )
}

// TODO: enable and fix this test when we fix the global config loading
// #[tokio::test]
// async fn default_pypi_config() {
//     let pixi = PixiControl::new().unwrap();
//     // Create new PyPI configuration
//     let index_url: Url = "https://pypi.org/simple".parse().unwrap();
//     let mut pypi_config = PyPIConfig::default();
//     pypi_config.index_url = Some(index_url.clone());
//     pypi_config.extra_index_urls = vec![index_url.clone()];
//     // pypi_config.keyring_provider = Some(pixi::config::KeyringProvider::Subprocess);
//     let mut config = Config::default();
//     config.pypi_config = pypi_config;
//     pixi.init().await.unwrap();

//     // Load the project
//     let project = pixi.project().unwrap();
//     let options = project.environment("default").unwrap().pypi_options();
//     assert_eq!(options.index_url, Some(index_url.clone()));
//     assert_eq!(options.extra_index_urls, Some(vec![index_url]));

//     assert_eq!(
//         project.config().pypi_config().keyring_provider,
//         Some(pixi::config::KeyringProvider::Subprocess)
//     );
// }
