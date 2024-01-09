mod common;

use crate::common::package_database::{Package, PackageDatabase};
use crate::common::LockFileExt;
use crate::common::PixiControl;
use pixi::project::{DependencyType, SpecType};
use rattler_conda_types::{PackageName, Platform};
use std::str::FromStr;
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
        .set_type(DependencyType::CondaDependency(SpecType::Host))
        .await
        .unwrap();
    pixi.add("rattler==3")
        .set_type(DependencyType::CondaDependency(SpecType::Build))
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
        .set_type(DependencyType::CondaDependency(SpecType::Host))
        .await
        .unwrap();
    pixi.add("libidk")
        .set_type(DependencyType::CondaDependency(SpecType::Build))
        .await
        .unwrap();

    // Toml should contain the correct sections
    // We test if the toml file that is saved is correct
    // by checking if we get the correct values back in the manifest
    // We know this works because we test the manifest in another test
    // Where we check if the sections are put in the correct variables
    let project = pixi.project().unwrap();

    // Should contain all added dependencies
    let dependencies = project.dependencies(Some(SpecType::Run), Some(Platform::current()));
    let (name, _) = dependencies.into_specs().next().unwrap();
    assert_eq!(name, PackageName::try_from("rattler").unwrap());
    let host_deps = project.dependencies(Some(SpecType::Host), Some(Platform::current()));
    let (name, _) = host_deps.into_specs().next().unwrap();
    assert_eq!(name, PackageName::try_from("libcomputer").unwrap());
    let build_deps = project.dependencies(Some(SpecType::Build), Some(Platform::current()));
    let (name, _) = build_deps.into_specs().next().unwrap();
    assert_eq!(name, PackageName::try_from("libidk").unwrap());

    // Lock file should contain all packages as well
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec("rattler==1"));
    assert!(lock.contains_matchspec("libcomputer==1.2"));
    assert!(lock.contains_matchspec("libidk==3.1"));
}

/// Test adding a package for a specific OS
#[tokio::test]
async fn add_functionality_os() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(
        Package::build("rattler", "1")
            .with_subdir(Platform::LinuxS390X)
            .finish(),
    );

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
    pixi.add("rattler==1")
        .set_platforms(&[Platform::LinuxS390X])
        .set_type(DependencyType::CondaDependency(SpecType::Host))
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_matchspec_for_platform("rattler==1", Platform::LinuxS390X));
}

/// Test the `pixi add --pypi` functionality
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn add_pypi_functionality() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(Package::build("python", "3.9").finish());

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

    // Add python
    pixi.add("python")
        .set_type(DependencyType::CondaDependency(SpecType::Run))
        .with_install(false)
        .await
        .unwrap();

    // Add a pypi package
    pixi.add("pipx")
        .set_type(DependencyType::PypiDependency)
        .with_install(false)
        .await
        .unwrap();

    // Add a pypi package to a target
    pixi.add("boto3>=1.33")
        .set_type(DependencyType::PypiDependency)
        .with_install(false)
        .set_platforms(&[Platform::Osx64])
        .await
        .unwrap();

    // Add a pypi package to a target
    pixi.add("pytest[all]")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .with_install(false)
        .await
        .unwrap();

    pixi.add("requests [security,tests] >= 2.8.1, == 2.8.*")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .with_install(false)
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_package(&PackageName::from_str("pipx").unwrap()));
    assert!(lock.contains_pep508_requirement_for_platform(
        pep508_rs::Requirement::from_str("boto3>=1.33").unwrap(),
        Platform::Osx64
    ));
    assert!(lock.contains_pep508_requirement_for_platform(
        pep508_rs::Requirement::from_str("pytest[all]").unwrap(),
        Platform::Linux64
    ));
    assert!(lock.contains_pep508_requirement_for_platform(
        pep508_rs::Requirement::from_str("requests [security,tests] >= 2.8.1, == 2.8.*").unwrap(),
        Platform::Linux64
    ));
}
