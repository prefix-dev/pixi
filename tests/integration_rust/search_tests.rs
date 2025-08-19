use insta::assert_snapshot;
use pixi_cli::search;
use rattler_conda_types::Platform;
use tempfile::TempDir;
use url::Url;

use crate::common::{
    PixiControl,
    package_database::{Package, PackageDatabase},
};
use crate::setup_tracing;

#[tokio::test]
async fn search_return_latest_across_everything() {
    setup_tracing();

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

#[tokio::test]
async fn search_using_match_spec() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add a package `foo` with different versions and different builds
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_0")
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_1")
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_0")
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_1")
            .finish(),
    );

    // Write the repodata to disk
    let temp_dir = TempDir::new().unwrap();
    let channel_dir = temp_dir.path().join("channel");
    package_database.write_repodata(&channel_dir).await.unwrap();
    let channel = Url::from_file_path(channel_dir).unwrap();
    let platform = Platform::current();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-search-using-match-spec"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    "#
    ))
    .unwrap();

    // Without match spec the latest version is returned
    let binding = pixi.search("foo".to_string()).await.unwrap().unwrap();
    let found_package = binding.last().unwrap();
    assert_eq!(found_package.package_record.version.as_str(), "0.2.0");
    assert_eq!(found_package.package_record.build.as_str(), "h60d57d3_1");

    // Search for a specific version
    let binding = pixi
        .search("foo<=0.1.0".to_string())
        .await
        .unwrap()
        .unwrap();
    let found_package = binding.last().unwrap();
    assert_eq!(found_package.package_record.version.as_str(), "0.1.0");
    assert_eq!(found_package.package_record.build.as_str(), "h60d57d3_1");

    // Search for a specific build
    let binding = pixi
        .search("foo[build=h60d57d3_0]".to_string())
        .await
        .unwrap()
        .unwrap();
    let found_package = binding.last().unwrap();
    assert_eq!(found_package.package_record.version.as_str(), "0.2.0");
    assert_eq!(found_package.package_record.build.as_str(), "h60d57d3_0");
}

#[tokio::test]
async fn test_search_multiple_versions() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add package with multiple versions and build strings
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_0")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_1")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_0")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_1")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    let temp_dir = TempDir::new().unwrap();
    let channel_dir = temp_dir.path().join("channel");
    package_database.write_repodata(&channel_dir).await.unwrap();
    let channel = Url::from_file_path(channel_dir).unwrap();
    let platform = Platform::current();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-multiple-versions"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    "#
    ))
    .unwrap();

    let mut out = Vec::new();
    let builder = pixi.search("foo".to_string());
    let result = search::execute_impl(builder.args, &mut out)
        .await
        .unwrap()
        .unwrap();
    let output = String::from_utf8(out).unwrap();
    let output = output
        // Remove ANSI escape codes from output
        .replace("\x1b[0m", "")
        .replace("\x1b[1m", "")
        .replace("\x1b[2m", "");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].package_record.version.as_str(), "0.2.0");
    assert_eq!(result[0].package_record.build, "h60d57d3_1");
    let output = output
        .lines()
        // Filter out URL line since temporary directory name is random.
        .filter(|line| !line.starts_with("URL"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!(output);
}
