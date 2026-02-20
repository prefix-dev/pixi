use insta::assert_snapshot;
use pixi_cli::search;
use rattler_conda_types::Platform;
use serde_json::Value;
use tempfile::TempDir;
use url::Url;

use crate::common::PixiControl;
use crate::setup_tracing;
use pixi_test_utils::{MockRepoData, Package};

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC [ ... m sequences
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == 'm' {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[tokio::test]
async fn search_return_latest_across_everything() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

    // Add a package `foo` with 4 different versions, on different platforms
    // and different channels
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("foo", "2").finish());

    package_database.add_package(
        Package::build("foo", "3")
            .with_subdir(Platform::current())
            .finish(),
    );

    let mut latest_package_database = MockRepoData::default();
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

    let mut package_database = MockRepoData::default();

    // Add a package `foo` with different versions and different builds
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_0")
            .with_build_number(0)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_1")
            .with_build_number(1)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_0")
            .with_build_number(0)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_1")
            .with_build_number(1)
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

    let mut package_database = MockRepoData::default();

    // Add package with multiple versions and build strings
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_0")
            .with_build_number(0)
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.1.0")
            .with_build("h60d57d3_1")
            .with_build_number(1)
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_0")
            .with_build_number(0)
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "0.2.0")
            .with_build("h60d57d3_1")
            .with_build_number(1)
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
    let output = strip_ansi(&String::from_utf8(out).unwrap());

    let latest_package = result.last().expect("should have at least one result");
    assert_eq!(latest_package.package_record.version.as_str(), "0.2.0");
    assert_eq!(latest_package.package_record.build, "h60d57d3_1");
    let output = output
        .lines()
        // Filter out URL line since temporary directory name is random.
        .filter(|line| !line.starts_with("URL"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!(output);
    assert!(output.contains("0.1.0    h60d57d3_1  (+ 1 build)"));
}

#[tokio::test]
async fn test_search_multiple_packages_compact_view() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

    // Add multiple different packages
    package_database.add_package(
        Package::build("alpha", "1.0.0")
            .with_build("h1_0")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("alpha", "2.0.0")
            .with_build("h1_0")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("alpha", "3.0.0")
            .with_build("h1_0")
            .with_subdir(Platform::NoArch)
            .finish(),
    );
    package_database.add_package(
        Package::build("beta", "0.5.0")
            .with_build("h2_0")
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
    name = "test-compact-view"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    "#
    ))
    .unwrap();

    let mut out = Vec::new();
    let mut builder = pixi.search("*a*".to_string());
    builder.args.limit = 2;
    builder.args.limit_packages = 10;
    let _result = search::execute_impl(builder.args, &mut out)
        .await
        .unwrap()
        .unwrap();
    let output = strip_ansi(&String::from_utf8(out).unwrap());

    // Filter out lines containing temp dir paths
    let output = output
        .lines()
        .map(|line| {
            // Replace channel URLs (file:///...) with a placeholder
            if let Some(idx) = line.find("file:///") {
                format!("{}<channel>", &line[..idx])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!(output);
}

#[tokio::test]
async fn test_search_json_output() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

    // Add packages on different platforms with different attributes
    package_database.add_package(
        Package::build("foo", "1.0.0")
            .with_build("h1_0")
            .with_subdir(Platform::NoArch)
            .with_dependency("bar >=1.0")
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "2.0.0")
            .with_build("h2_0")
            .with_subdir(Platform::current())
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
    name = "test-json-output"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    "#
    ))
    .unwrap();

    let mut out = Vec::new();
    let mut builder = pixi.search("foo".to_string());
    builder.args.json = true;
    let _result = search::execute_impl(builder.args, &mut out)
        .await
        .unwrap()
        .unwrap();
    let output = String::from_utf8(out).unwrap();

    // Parse the JSON output to verify structure
    let json: Value = serde_json::from_str(&output).expect("output should be valid JSON");
    let obj = json.as_object().expect("top level should be an object");

    // Should have platform keys
    assert!(
        obj.contains_key("noarch") || obj.contains_key(&platform.to_string()),
        "should contain platform keys, got: {:?}",
        obj.keys().collect::<Vec<_>>()
    );

    // Each platform value should be an array of records
    let first_platform_records = obj.values().next().unwrap().as_array().unwrap();
    assert!(!first_platform_records.is_empty());

    // Verify a record has the expected fields
    let first_record = first_platform_records[0].as_object().unwrap();
    assert!(first_record.contains_key("name"));
    assert!(first_record.contains_key("version"));
    assert!(first_record.contains_key("build"));
    assert!(first_record.contains_key("build_number"));
    assert!(first_record.contains_key("url"));
    assert!(first_record.contains_key("depends"));
    assert!(first_record.contains_key("fn"));
}
