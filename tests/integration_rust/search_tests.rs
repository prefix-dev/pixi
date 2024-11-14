use rattler_conda_types::Platform;
use tempfile::TempDir;
use url::Url;

use crate::common::{
    package_database::{Package, PackageDatabase},
    PixiControl,
};

#[tokio::test]
async fn search_return_latest_across_everything() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` with 4 different versions, on different platforms
    // and different channels
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("foo", "2").finish());

    package_database.add_package(
        Package::build("foo", "3")
            .with_subdir(Platform::current())
            .finish(),
    );

    let mut latest_package_database = PackageDatabase::default();
    latest_package_database.add_package(
        Package::build("foo", "4")
            .with_subdir(Platform::current())
            .finish(),
    );

    // Write the repodata to disk
    let channel_base_dir = TempDir::new().unwrap();
    let not_latest_channel_dir = channel_base_dir.path().join("not-latest");
    let latest_channel_dir = channel_base_dir.path().join("latest");

    package_database
        .write_repodata(&not_latest_channel_dir)
        .await
        .unwrap();

    latest_package_database
        .write_repodata(&latest_channel_dir)
        .await
        .unwrap();

    let channel_latest = Url::from_file_path(latest_channel_dir).unwrap();
    let channel_not_latest = Url::from_file_path(not_latest_channel_dir).unwrap();

    let platform = Platform::current();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-solve-group"
    channels = ["{channel_latest}", "{channel_not_latest}"]
    platforms = ["{platform}"]

    "#
    ))
    .unwrap();

    // Search and check that the latest version is returned
    let binding = pixi.search("foo".to_string()).await.unwrap().unwrap();
    let found_package = binding.last().unwrap();

    assert_eq!(found_package.package_record.version.as_str(), "4");
}
