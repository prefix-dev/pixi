use fs_err as fs;
use rattler_conda_types::Platform;
use tempfile::TempDir;

use crate::common::{
    PixiControl,
    package_database::{Package, PackageDatabase},
};
use crate::setup_tracing;

/// Test that verifies build backend receives the correct resolved source path
/// when a relative path is specified in the source field
#[tokio::test]
async fn test_build_with_relative_source_path() {
    setup_tracing();

    // Create a simple package database for our test
    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("empty-backend", "0.1.0").finish());

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Create a PixiControl instance and initialize it
    let pixi = PixiControl::new().unwrap();

    // Create a relative source directory structure outside the workspace
    let alternative_source_dir = pixi
        .workspace_path()
        .parent()
        .unwrap()
        .join("alternative-source");
    fs::create_dir_all(&alternative_source_dir).unwrap();

    // Create a simple recipe.yaml in the alternative source
    let recipe_content = r#"
schema_version: 1

package:
  name: test-package
  version: 0.1.0

build:
  number: 0
  noarch: generic

about:
  summary: Test package for relative source path
"#;
    fs::write(alternative_source_dir.join("recipe.yaml"), recipe_content).unwrap();

    // Create a manifest with relative source path
    let manifest_content = format!(
        r#"
[package]
name = "test-package"
version = "0.1.0"
description = "Test package for relative source path"

[package.build]
backend = {{ name = "empty-backend", version = "0.1.0" }}
channels = [
  "file://{}"
]
source.path = "../alternative-source"

[workspace]
channels = [
  "file://{}"
]
platforms = ["{}"]
preview = ["pixi-build"]
"#,
        channel_dir
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        channel_dir
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        Platform::current()
    );

    // Write the manifest
    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Test that the manifest can be loaded and the source path resolves correctly
    let workspace = dbg!(pixi.workspace()).unwrap();

    if let Some(package) = &workspace.package {
        if let Some(source_spec) = &package.value.build.source {
            match &source_spec {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    // Test that the path resolves to the correct absolute location
                    let resolved_path = path_spec.resolve(pixi.workspace_path()).unwrap();
                    let expected_path = alternative_source_dir.canonicalize().unwrap();
                    let resolved_canonical = resolved_path.canonicalize().unwrap();

                    assert_eq!(
                        resolved_canonical, expected_path,
                        "Resolved path should point to the alternative source directory"
                    );

                    // Verify the recipe.yaml exists at the resolved location
                    assert!(
                        resolved_path.join("recipe.yaml").exists(),
                        "recipe.yaml should exist at the resolved source path"
                    );

                    // Test that the original relative path is preserved in the spec
                    assert_eq!(path_spec.path.as_str(), "../alternative-source");
                }
                _ => panic!("Expected a path source spec"),
            }
        } else {
            panic!("Expected source field to be present in build config");
        }
    } else {
        panic!("Expected package manifest to be present");
    }
}

/// Test that verifies absolute paths work correctly
#[tokio::test]
async fn test_build_with_absolute_source_path() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("empty-backend", "0.1.0").finish());

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create an absolute source directory
    let absolute_source_dir = pixi.workspace_path().join("absolute-source");
    fs::create_dir_all(&absolute_source_dir).unwrap();
    fs::write(
        absolute_source_dir.join("recipe.yaml"),
        "schema_version: 1\n",
    )
    .unwrap();

    let manifest_content = format!(
        r#"
[package]
name = "test-package-abs"
version = "0.1.0"

[package.build]
backend = {{ name = "empty-backend", version = "0.1.0" }}
channels = ["file://{}"]
source.path = "{}"

[workspace]
channels = ["file://{}"]
platforms = ["{}"]
preview = ["pixi-build"]
"#,
        channel_dir
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        absolute_source_dir
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        channel_dir
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    let workspace = dbg!(pixi.workspace()).unwrap();

    if let Some(package) = &workspace.package {
        if let Some(source_spec) = &package.value.build.source {
            match &source_spec {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    let resolved_path = path_spec.resolve(pixi.workspace_path()).unwrap();
                    let expected_path = absolute_source_dir.canonicalize().unwrap();
                    let resolved_canonical = resolved_path.canonicalize().unwrap();

                    assert_eq!(resolved_canonical, expected_path);
                    assert!(resolved_path.join("recipe.yaml").exists());
                }
                _ => panic!("Expected a path source spec"),
            }
        }
    }
}

/// Test that verifies subdirectory relative paths work correctly
#[tokio::test]
async fn test_build_with_subdirectory_source_path() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("empty-backend", "0.1.0").finish());

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create a subdirectory source path
    let subdir_source = pixi.workspace_path().join("subdir").join("source");
    fs::create_dir_all(&subdir_source).unwrap();
    fs::write(subdir_source.join("recipe.yaml"), "schema_version: 1\n").unwrap();

    let manifest_content = format!(
        r#"
[package]
name = "test-package-subdir"
version = "0.1.0"

[package.build]
backend = {{ name = "empty-backend", version = "0.1.0" }}
channels = ["file://{}"]
source.path = "./subdir/source"

[workspace]
channels = ["file://{}"]
platforms = ["{}"]
preview = ["pixi-build"]
"#,
        channel_dir
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        channel_dir
            .path()
            .display()
            .to_string()
            .replace('\\', "\\\\"),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    let workspace = pixi.workspace().unwrap();

    if let Some(package) = &workspace.package {
        if let Some(source_spec) = &package.value.build.source {
            match &source_spec {
                pixi_spec::SourceLocationSpec::Path(path_spec) => {
                    // Test that the original relative path is preserved
                    assert_eq!(path_spec.path.as_str(), "./subdir/source");

                    // Test that it resolves to the correct absolute location
                    let resolved_path = path_spec.resolve(pixi.workspace_path()).unwrap();
                    assert!(resolved_path.is_absolute());
                    assert!(resolved_path.join("recipe.yaml").exists());

                    // Verify the resolved path matches our expectation
                    let expected_path = subdir_source.canonicalize().unwrap();
                    let resolved_canonical = resolved_path.canonicalize().unwrap();
                    assert_eq!(resolved_canonical, expected_path);
                }
                _ => panic!("Expected a path source spec"),
            }
        }
    }
}
