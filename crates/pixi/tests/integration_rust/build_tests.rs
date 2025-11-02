use fs_err as fs;
use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::BackendOverride;
use pixi_consts::consts;
use rattler_conda_types::Platform;
use tempfile::TempDir;

use crate::{
    common::{
        LockFileExt, PixiControl,
        package_database::{Package, PackageDatabase},
    },
    setup_tracing,
};

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

/// Test that demonstrates using PassthroughBackend with PixiControl
/// to test build operations without requiring actual backend processes.
#[tokio::test]
async fn test_with_passthrough_backend() {
    setup_tracing();

    // Create a PixiControl instance with PassthroughBackend
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a simple source directory
    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();

    // Create a pixi.toml that the PassthroughBackend will read
    let pixi_toml_content = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#;
    fs::write(source_dir.join("pixi.toml"), pixi_toml_content).unwrap();

    // Create a manifest with a source dependency
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
# This will use the PassthroughBackend instead of a real backend
my-package = {{ path = "./my-package" }}
"#,
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Build the lock-file and ensure that it contains our package.
    let lock_file = pixi.update_lock_file().await.unwrap();
    assert!(lock_file.contains_conda_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "my-package",
    ));
}

/// Test that verifies [package.build] source.path is resolved relative to the
/// package manifest directory, not the workspace root.
///
/// This tests the fix for out-of-tree builds where a package manifest
/// specifies `source.path = "src"` and expects it to be resolved relative
/// to the package manifest's parent directory.
#[tokio::test]
async fn test_package_build_source_relative_to_manifest() {
    setup_tracing();

    // Create a PixiControl instance with PassthroughBackend
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create the package structure:
    // workspace/
    //   pixi.toml (workspace and package manifest)
    //   src/      (source directory - should be found relative to package manifest)
    //     pixi.toml (build source manifest)

    let package_source_dir = pixi.workspace_path().join("src");
    fs::create_dir_all(&package_source_dir).unwrap();

    // Create a pixi.toml in the source directory that PassthroughBackend will read
    let source_pixi_toml = r#"
[package]
name = "test-build-source"
version = "0.1.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#;
    fs::write(package_source_dir.join("pixi.toml"), source_pixi_toml).unwrap();

    // Create a manifest where the package has [package.build] with source.path
    // The source.path should be resolved relative to the package manifest directory
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[package]
name = "test-build-source"
version = "0.1.0"
description = "Test package for build source path resolution"

[package.build]
backend = {{ name = "in-memory", version = "0.1.0" }}
# This should resolve to <package_manifest_dir>/src, not <workspace_root>/src
source.path = "src"

[dependencies]
test-build-source = {{ path = "." }}
"#,
        Platform::current(),
    );

    // Write the manifest
    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Actually trigger the build process to test the bug
    // This will call build_backend_metadata which uses alternative_root
    let result = pixi.update_lock_file().await;

    // The test should succeed if the source path is resolved correctly
    // If the bug exists (manifest_path instead of manifest_path.parent()),
    // the build will fail because it will try to find src relative to pixi.toml (a file)
    // instead of relative to the directory containing pixi.toml
    assert!(
        result.is_ok(),
        "Lock file update should succeed when source.path is resolved correctly. Error: {:?}",
        result.err()
    );

    let lock_file = result.unwrap();

    // Verify the package was built and is in the lock file
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "test-build-source",
        ),
        "Built package should be in the lock file"
    );
}
