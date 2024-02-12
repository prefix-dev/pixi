use crate::common::{
    package_database::{Package, PackageDatabase},
    LockFileExt, PixiControl,
};
use rattler_conda_types::Platform;
use tempfile::TempDir;
use url::Url;

mod common;

#[tokio::test]
async fn solve_group_functionality() {
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
