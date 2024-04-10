use std::str::FromStr;

use crate::common::{
    package_database::{Package, PackageDatabase},
    LockFileExt, PixiControl,
};
use rattler_conda_types::{PackageName, Platform};
use rattler_lock::DEFAULT_ENVIRONMENT_NAME;
use serial_test::serial;
use tempfile::TempDir;
use url::Url;

mod common;

#[tokio::test]
async fn conda_solve_group_functionality() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` with 3 different versions
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database.add_package(Package::build("foo", "3").finish());

    // Add a package `bar` with 1 version that restricts `foo` to version 2 or lower.
    package_database.add_package(
        Package::build("bar", "1")
            .with_dependency("foo <3")
            .finish(),
    );

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let channel = Url::from_file_path(channel_dir.path()).unwrap();
    let platform = Platform::current();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-solve-group"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    [dependencies]
    foo = "*"

    [feature.test.dependencies]
    bar = "*"

    [environments]
    prod = {{ solve-group = "prod" }}
    test = {{ features=["test"], solve-group = "prod" }}
    "#
    ))
    .unwrap();

    // Get an up-to-date lockfile
    let lock_file = pixi.up_to_date_lock_file().await.unwrap();

    assert!(
        lock_file.contains_match_spec("default", platform, "foo ==3"),
        "default should have the highest version of foo"
    );
    assert!(
        !lock_file.contains_match_spec("default", platform, "bar"),
        "default should not contain bar"
    );

    assert!(
        lock_file.contains_match_spec("prod", platform, "foo ==2"),
        "prod should have foo==2 because it shares the solve group with test"
    );
    assert!(
        !lock_file.contains_match_spec("prod", platform, "bar"),
        "prod should not contain bar"
    );

    assert!(
        lock_file.contains_match_spec("test", platform, "foo ==2"),
        "test should have foo==2 because bar depends on foo <3"
    );
    assert!(
        lock_file.contains_match_spec("test", platform, "bar"),
        "test should contain bar"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
// #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_purl_are_added_for_pypi() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("boltons").with_install(true).await.unwrap();

    let lock_file = pixi.up_to_date_lock_file().await.unwrap();

    // Check if boltons has a purl
    lock_file
        .default_environment()
        .unwrap()
        .packages(Platform::current())
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().package_record().name
                == PackageName::from_str("boltons").unwrap()
            {
                assert!(dep.as_conda().unwrap().package_record().purls.is_empty());
            }
        });

    // Add boltons from pypi
    pixi.add("boltons")
        .with_install(true)
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    let lock_file = pixi.up_to_date_lock_file().await.unwrap();

    // Check if boltons has a purl
    lock_file
        .default_environment()
        .unwrap()
        .packages(Platform::current())
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().package_record().name
                == PackageName::from_str("boltons").unwrap()
            {
                assert!(!dep.as_conda().unwrap().package_record().purls.is_empty());
            }
        });

    // Check if boltons exists only as conda dependency
    assert!(lock_file.contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "boltons"
    ));
    assert!(!lock_file.contains_pypi_package(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "boltons"
    ));
}
