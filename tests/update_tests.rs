mod common;

use pixi_consts::consts;
use rattler_conda_types::Platform;
use tempfile::TempDir;

use crate::common::{
    package_database::{Package, PackageDatabase},
    LockFileExt, PixiControl,
};

#[tokio::test]
async fn test_update() {
    let mut package_database = PackageDatabase::default();

    // Add a package
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("foo", "1").finish());

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a dependency on `bar`
    pixi.add("bar <=2").await.unwrap();
    pixi.add("foo <=2").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "bar ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==1"
    ));

    // Add version 2 and 3 of `bar`. Version 3 should never be selected.
    package_database.add_package(Package::build("bar", "2").finish());
    package_database.add_package(Package::build("bar", "3").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database.add_package(Package::build("foo", "3").finish());
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Run the update command to update all the packages
    pixi.update().await.unwrap();

    // Reload the lock-file and check if the new version of `bar` still matches the
    // spec and has been updated.
    let lock = pixi.lock_file().await.unwrap();
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we updated the lock-file"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "bar ==2"
        ),
        "expected `bar` to be on version 2 because we updated the lock-file"
    );
}

#[tokio::test]
async fn test_update_single_package() {
    let mut package_database = PackageDatabase::default();

    // Add packages
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("foo", "1").finish());

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a dependency on `bar`
    pixi.add("bar <=2").await.unwrap();
    pixi.add("foo <=2").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "bar ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==1"
    ));

    // Add version 2 and 3 of `bar`. Version 3 should never be selected.
    package_database.add_package(Package::build("bar", "2").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Run the update command to update a single package
    pixi.update().with_package("foo").await.unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we updated it"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "bar ==1"
        ),
        "expected `bar` to be on version 1 because only foo should be updated"
    );
}

// #[tokio::test]
// async fn test_update_single_environment() {
//     let mut package_database = PackageDatabase::default();
//
//     // Add packages
//     package_database.add_package(Package::build("foo", "1").finish());
//
//     // Write the repodata to disk
//     let channel_dir = TempDir::new().unwrap();
//     package_database
//         .write_repodata(channel_dir.path())
//         .await
//         .unwrap();
//
//     let pixi = PixiControl::new().unwrap();
//
//     // Create a new project using our package database.
//     pixi.init()
//         .with_local_channel(channel_dir.path())
//         .await
//         .unwrap();
//
//     // Add a dependency on `bar`
//     pixi.add("foo <=2").await.unwrap();
//
//     // Get the created lock-file
//     let lock = pixi.lock_file().await.unwrap();
//     assert!(lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "bar ==1"));
//     assert!(lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "foo ==1"));
//
//     // Add version 2 and 3 of `bar`. Version 3 should never be selected.
//     package_database.add_package(Package::build("bar", "2").finish());
//     package_database.add_package(Package::build("foo", "2").finish());
//     package_database
//         .write_repodata(channel_dir.path())
//         .await
//         .unwrap();
//
//     // Run the update command to update a single package
//     pixi.update().with_package("foo").await.unwrap();
//
//     let lock = pixi.lock_file().await.unwrap();
//     assert!(
//         lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "foo ==2"),
//         "expected `foo` to be on version 2 because we updated it"
//     );
//     assert!(
//         lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "bar ==1"),
//         "expected `bar` to be on version 1 because only foo should be updated"
//     );
// }
