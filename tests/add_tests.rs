mod common;
use crate::common::package_database::{Package, PackageDatabase};
use crate::common::LockFileExt;
use crate::common::PixiControl;
use pixi::cli::add::SpecType;
use tempfile::TempDir;

/// Test add functionality for different types of packages.
/// Run, dev, build
#[tokio::test]
async fn add_functionality() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(Package::build("rattler", "1").finish());
    package_database.add_package(Package::build("rattler", "2").finish());
    package_database.add_package(Package::build("rattler", "3").finish());

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a package
    pixi.add("rattler==1").await.unwrap();
    pixi.add("rattler==2")
        .set_type(SpecType::Host)
        .await
        .unwrap();
    pixi.add("rattler==3")
        .set_type(SpecType::Build)
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("rattler==3"));
    assert!(!lock.contains_matchspec("rattler==2"));
    assert!(!lock.contains_matchspec("rattler==1"));
}

/// Test that we get the union of all packages in the lockfile for the run, build and host
#[tokio::test]
async fn add_functionality_union() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(Package::build("rattler", "1").finish());
    package_database.add_package(Package::build("libcomputer", "1.2").finish());
    package_database.add_package(Package::build("libidk", "3.1").finish());

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a package
    pixi.add("rattler").await.unwrap();
    pixi.add("libcomputer")
        .set_type(SpecType::Host)
        .await
        .unwrap();
    pixi.add("libidk").set_type(SpecType::Build).await.unwrap();

    // Lock file should contain all packages
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("rattler==1"));
    assert!(lock.contains_matchspec("libcomputer==1.2"));
    assert!(lock.contains_matchspec("libidk==3.1"));
}
