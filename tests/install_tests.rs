mod common;

use crate::common::repodata::{ChannelBuilder, PackageBuilder, SubdirBuilder};
use common::{LockFileExt, PixiControl};
use pixi::Project;
use rattler_conda_types::{Platform, Version};
use std::str::FromStr;
use tempfile::TempDir;

/// Should add a python version to the environment and lock file that matches the specified version
/// and run it
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_run_python() {
    let mut pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.add(["python==3.11.0"]).await.unwrap();

    // Check if lock has python version
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("python==3.11.0"));

    // Check if python is installed and can be run
    let result = pixi.run(["python", "--version"]).await.unwrap();
    assert!(result.success());
    assert_eq!(result.stdout(), "Python 3.11.0\n");
}

#[tokio::test]
async fn init_creates_project_manifest() {
    let tmp_dir = TempDir::new().unwrap();

    // Run the init command
    pixi::cli::init::execute(pixi::cli::init::Args {
        path: tmp_dir.path().to_path_buf(),
    })
    .await
    .unwrap();

    // There should be a loadable project manifest in the directory
    let project = Project::load(&tmp_dir.path().join(pixi::consts::PROJECT_MANIFEST)).unwrap();

    // Default configuration should be present in the file
    assert!(!project.name().is_empty());
    assert_eq!(project.version(), &Version::from_str("0.1.0").unwrap());
}

#[tokio::test]
async fn test_custom_channel() {
    let mut pixi = PixiControl::new().unwrap();

    // Create a new project
    pixi.init().await.unwrap();

    // Set the channel to something we created
    pixi.set_channel(
        ChannelBuilder::default().with_subdir(
            SubdirBuilder::new(Platform::current())
                .with_package(
                    PackageBuilder::new("foo", "1")
                        .with_dependency("bar >=1")
                        .with_build_string("helloworld"),
                )
                .with_package(PackageBuilder::new("bar", "1")),
        ),
    )
    .await
    .unwrap();

    // Add a dependency on `foo`
    pixi.add(["foo"]).await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("foo==1=helloworld"));
}

#[tokio::test]
async fn test_incremental_lock_file() {
    let mut pixi = PixiControl::new().unwrap();

    // Create a new project
    pixi.init().await.unwrap();

    // Set the channel to something we created
    pixi.set_channel(
        ChannelBuilder::default().with_subdir(
            SubdirBuilder::new(Platform::current())
                .with_package(
                    PackageBuilder::new("foo", "1")
                        .with_dependency("bar >=1")
                        .with_build_string("helloworld"),
                )
                .with_package(PackageBuilder::new("bar", "1")),
        ),
    )
    .await
    .unwrap();

    // Add a dependency on `foo`
    pixi.add(["foo"]).await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("foo==1=helloworld"));

    // Update the channel, add a version 2 for both foo and bar.
    pixi.set_channel(
        ChannelBuilder::default().with_subdir(
            SubdirBuilder::new(Platform::current())
                .with_package(
                    PackageBuilder::new("foo", "1")
                        .with_dependency("bar >=1")
                        .with_build_string("helloworld"),
                )
                .with_package(
                    PackageBuilder::new("foo", "2")
                        .with_dependency("bar >=1")
                        .with_build_string("awholenewworld"),
                )
                .with_package(PackageBuilder::new("bar", "1"))
                .with_package(PackageBuilder::new("bar", "2")),
        ),
    )
    .await
    .unwrap();

    // Change the dependency on `foo` to use version 2.
    pixi.add(["foo >=2"]).await.unwrap();

    // Get the created lock-file, `foo` should have been updated, but `bar` should remain on v1.
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("foo==2=awholenewworld"));
    assert!(lock.contains_matchspec("bar==1"));
}
