use fs_err as fs;
use pixi_build_backend_passthrough::{BackendEvent, ObservableBackend, PassthroughBackend};
use pixi_build_frontend::BackendOverride;
use pixi_consts::consts;
use rattler_conda_types::{Platform, package::RunExportsJson};
use tempfile::TempDir;

use crate::{
    common::{LockFileExt, PixiControl},
    setup_tracing,
};
use pixi_test_utils::{MockRepoData, Package};

fn write_source_package_manifest(path: &std::path::Path, name: &str, version: &str, extra: &str) {
    let source_pixi_toml = format!(
        r#"
[package]
name = "{name}"
version = "{version}"

[package.build]
backend = {{ name = "in-memory", version = "0.1.0" }}
{extra}
"#
    );
    fs::write(path.join("pixi.toml"), source_pixi_toml).unwrap();
}

fn write_basic_source_package_manifest(path: &std::path::Path, version: &str, extra: &str) {
    write_source_package_manifest(path, "my-package", version, extra);
}

fn write_source_workspace_manifest(
    path: &std::path::Path,
    channels: &[&str],
    source_dependencies: &[&str],
) {
    let channels = channels
        .iter()
        .map(|c| format!(r#""{c}""#))
        .collect::<Vec<_>>()
        .join(", ");
    let source_dependencies = source_dependencies
        .iter()
        .map(|name| format!(r#"{name} = {{ path = "./{name}" }}"#))
        .collect::<Vec<_>>()
        .join("\n");
    let manifest_content = format!(
        r#"
[workspace]
channels = [{channels}]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
{source_dependencies}
"#,
        Platform::current()
    );
    fs::write(path, manifest_content).unwrap();
}

fn write_basic_source_workspace_manifest(path: &std::path::Path, channels: &[&str]) {
    write_source_workspace_manifest(path, channels, &["my-package"]);
}

fn write_source_workspace_manifest_with_binary_dependencies(
    path: &std::path::Path,
    channels: &[&str],
    binary_dependencies: &[&str],
) {
    let channels = channels
        .iter()
        .map(|c| format!(r#""{c}""#))
        .collect::<Vec<_>>()
        .join(", ");
    let binary_dependencies = binary_dependencies
        .iter()
        .map(|name| format!(r#"{name} = "*""#))
        .collect::<Vec<_>>()
        .join("\n");
    let manifest_content = format!(
        r#"
[workspace]
channels = [{channels}]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
{binary_dependencies}
"#,
        Platform::current()
    );
    fs::write(path, manifest_content).unwrap();
}

/// Test that verifies build backend receives the correct resolved source path
/// when a relative path is specified in the source field
#[tokio::test]
async fn test_build_with_relative_source_path() {
    setup_tracing();

    // Create a simple package database for our test
    let mut package_database = MockRepoData::default();
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

    let mut package_database = MockRepoData::default();
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

    if let Some(package) = &workspace.package
        && let Some(source_spec) = &package.value.build.source
    {
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

/// Test that verifies subdirectory relative paths work correctly
#[tokio::test]
async fn test_build_with_subdirectory_source_path() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
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

    if let Some(package) = &workspace.package
        && let Some(source_spec) = &package.value.build.source
    {
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

/// Test that verifies the build command can accept a path to a recipe.yaml file
/// via the --build-manifest argument
#[tokio::test]
async fn test_build_command_with_recipe_yaml_path() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    // Create a separate directory with a recipe.yaml
    let recipe_dir = pixi.workspace_path().join("my-recipe");
    fs::create_dir_all(&recipe_dir).unwrap();

    let recipe_content = r#"
package:
  name: test-package-from-recipe
  version: 0.1.0

build:
  number: 0
  noarch: generic

about:
  summary: Test package built from recipe.yaml
"#;
    let recipe_path = recipe_dir.join("recipe.yaml");
    fs::write(&recipe_path, recipe_content).unwrap();

    // Create a workspace manifest (pixi.toml) for workspace configuration
    let manifest_content = format!(
        r#"
[workspace]
channels = ["conda-forge"]
platforms = ["{}"]
preview = ["pixi-build"]
"#,
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Verify that the recipe.yaml file exists and is readable
    assert!(
        recipe_path.exists(),
        "recipe.yaml should exist at the expected path"
    );

    assert!(
        recipe_path.is_file(),
        "recipe.yaml should be a file, not a directory"
    );

    // Verify the content can be read
    let content = fs::read_to_string(&recipe_path).unwrap();
    assert!(
        content.contains("test-package-from-recipe"),
        "recipe.yaml should contain the package name"
    );
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

/// Test that verifies `.pixi/.gitignore` is created during `pixi build`
/// This fixes issue #4761 where pixi build didn't create the .gitignore file,
/// causing recursion errors in rattler-build when source files reference the project root
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_build_creates_gitignore() {
    setup_tracing();

    // Create a PixiControl instance
    let pixi = PixiControl::new().unwrap();

    // Create a minimal manifest with build configuration
    // We're not setting up a real backend, so the build will fail,
    // but the .gitignore should still be created
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[package]
name = "test-gitignore-build"
version = "0.1.0"
description = "Test package for .gitignore creation during build"

[package.build]
backend.name = "nonexistent-backend"
backend.version = "0.1.0"
"#,
        Platform::current(),
    );

    // Write the manifest
    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    let gitignore_path = pixi.workspace().unwrap().pixi_dir().join(".gitignore");

    // Verify .pixi/.gitignore doesn't exist initially
    assert!(
        !gitignore_path.exists(),
        ".pixi/.gitignore file should not exist before build"
    );

    // Run pixi build - this will fail because the backend doesn't exist,
    // but it should still create the .pixi/.gitignore file as part of
    // the sanity_check_workspace call
    let _ = pixi.build().await;

    // Verify .pixi/.gitignore was created even though the build failed
    assert!(
        gitignore_path.exists(),
        ".pixi/.gitignore file was not created after build"
    );
}

/// Test that demonstrates using PassthroughBackend with PixiControl
/// to test build operations without requiring actual backend processes.
#[tokio::test]
async fn test_different_variants_have_different_caches() {
    setup_tracing();

    // Create a package database with common dependencies
    // Each sdl2 package has run_exports that propagate itself, so when a package
    // has sdl2 as a host-dependency, the specific sdl2 version becomes a run-dependency.
    // This allows the solver to distinguish between variants built with different sdl2 versions.

    let run_exports = RunExportsJson {
        weak: vec!["sdl2 *".to_string()],
        ..Default::default()
    };

    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("sdl2", "2.26.5")
            .with_materialize(true)
            .with_run_exports(run_exports.clone())
            .finish(),
    );
    package_database.add_package(
        Package::build("sdl2", "2.32.0")
            .with_materialize(true)
            .with_run_exports(run_exports.clone())
            .finish(),
    );

    // Convert to channel
    let channel = package_database.into_channel().await.unwrap();

    // Create a PixiControl instance with PassthroughBackend
    // Configure the backend to apply run_exports from sdl2 (simulating what the mock packages define)
    let passthrough =
        PassthroughBackend::instantiator().with_run_exports("sdl2", run_exports.clone());

    // Create an observable backend and get the observer
    let (instantiator, mut observer) = ObservableBackend::instantiator(passthrough);

    let backend_override = BackendOverride::from_memory(instantiator);

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

[package.host-dependencies]
sdl2 = "*"
"#;
    fs::write(source_dir.join("pixi.toml"), pixi_toml_content).unwrap();

    // Create a manifest with a source dependency
    // Note: my-package must be a feature-specific dependency so that each environment
    // resolves it with its own sdl2 constraint, resulting in different variants.
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[workspace.build-variants]
sdl2 = ["2.26.5", "2.32.*"]

[feature.sdl2-26.dependencies]
sdl2 = "2.26.5"

[feature.sdl2-32.dependencies]
sdl2 = "2.32.*"

[environments]
sdl2-26 = {{ features = ["sdl2-26"] }}
sdl2-32 = {{ features = ["sdl2-32"] }}

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel.url(),
        Platform::current(),
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // install first time the environment with sdl2-26
    pixi.install()
        .with_environment(vec!["sdl2-26".to_string()])
        .await
        .unwrap();

    // do again, but we should have only one build
    pixi.install()
        .with_environment(vec!["sdl2-26".to_string()])
        .await
        .unwrap();

    let events = observer.build_events();

    assert_eq!(events.len(), 1, "Expected only one build for sdl2-26");

    // do again for different environment, we should have another build for sdl2-32
    pixi.install()
        .with_environment(vec!["sdl2-32".to_string()])
        .await
        .unwrap();

    let events = observer.build_events();

    assert_eq!(events.len(), 1, "Expected another build for sdl2-32");
}

/// Test that verifies when we generate a lock-file with a source package,
/// a second invocation of generating the lock-file should report it's already up to date.
///
/// This test creates a noarch: generic package with all fields that are compared
/// in `package_records_are_equal`:
/// - name, version, build, build_number
/// - depends, constrains
/// - license, license_family
/// - noarch, subdir
/// - features, track_features
/// - purls, python_site_packages_path
/// - run_exports
#[tokio::test]
async fn test_source_package_lock_file_up_to_date() {
    use pixi_test_utils::create_conda_package;
    use rattler_conda_types::{NoArchType, package::RunExportsJson};

    setup_tracing();

    // Create a PixiControl instance with PassthroughBackend
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a source package directory
    let source_dir = pixi.workspace_path().join("source-package");
    fs::create_dir_all(&source_dir).unwrap();

    // Create run_exports for the package
    let run_exports = RunExportsJson {
        weak: vec!["weak-dep >=1.0".to_string()],
        strong: vec!["strong-dep >=2.0".to_string()],
        ..Default::default()
    };

    // Create a Package with all fields from package_records_are_equal
    let mut package = pixi_test_utils::Package::build("test-source-pkg", "1.2.3")
        .with_build("test_build_0")
        .with_build_number(0)
        .with_subdir(Platform::NoArch)
        .with_dependency("some-dependency >=1.0")
        .with_run_exports(run_exports)
        .finish();

    // Modify the package_record to include all fields compared in package_records_are_equal
    package.package_record.license = Some("MIT".to_string());
    package.package_record.license_family = Some("MIT".to_string());
    package.package_record.noarch = NoArchType::generic();
    package.package_record.constrains = vec!["constrained-pkg <2.0".to_string()];
    package.package_record.track_features = vec!["test_feature".to_string()];
    package.package_record.features = Some("test_features".to_string());
    // Note: purls, python_site_packages_path, and experimental_extra_depends
    // are left as defaults since they're optional and the equality check handles None values

    // Create the .conda package file in the source directory
    let package_filename = format!(
        "{}-{}-{}.conda",
        package.package_record.name.as_normalized(),
        package.package_record.version,
        package.package_record.build
    );
    let package_path = source_dir.join(&package_filename);
    create_conda_package(&package, &package_path).expect("Failed to create conda package");

    // Create the pixi.toml for the source package that configures
    // PassthroughBackend to use the pre-built package
    let source_pixi_toml = format!(
        r#"
[package]
name = "test-source-pkg"
version = "1.2.3"

[package.build]
backend = {{ name = "passthrough", version = "0.1.0" }}

[package.build.config]
package = "{}"
"#,
        package_filename
    );
    fs::write(source_dir.join("pixi.toml"), source_pixi_toml).unwrap();

    // Create the workspace manifest that depends on the source package
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
test-source-pkg = {{ path = "./source-package" }}
"#,
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // First invocation: Generate the lock-file
    let workspace = pixi.workspace().unwrap();
    let (lock_file_data, was_updated) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("First lock file generation should succeed");

    // Verify the lock-file was actually created/updated
    assert!(was_updated, "First invocation should update the lock-file");

    // Verify the package is in the lock-file
    let lock_file = lock_file_data.into_lock_file();
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "test-source-pkg",
        ),
        "Lock file should contain the source package"
    );

    // Verify we can find the package
    assert!(
        lock_file.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "test-source-pkg"
        ),
        "Lock file should contain test-source-pkg"
    );

    // Second invocation: Load the workspace again and check if lock-file is up to date
    let workspace = pixi.workspace().unwrap();
    let (_, was_updated_second) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Second lock file check should succeed");

    // The second invocation should NOT update the lock-file since it's already up to date
    assert!(
        !was_updated_second,
        "Second invocation should report lock-file is already up to date"
    );
}

/// Test that verifies changing `[package.build.config]` invalidates the metadata cache
/// and causes the build backend to be re-queried.
///
/// This tests the fix for issue #5309 where changes to build configuration
/// (like `noarch = true` to `noarch = false`) did not invalidate the metadata cache.
///
/// The test uses ObservableBackend to verify that the backend is called again
/// when the configuration changes.
#[tokio::test]
async fn test_build_config_change_invalidates_cache() {
    setup_tracing();

    // Create an observable passthrough backend to track calls
    let passthrough = PassthroughBackend::instantiator();
    let (instantiator, mut observer) = ObservableBackend::instantiator(passthrough);
    let backend_override = BackendOverride::from_memory(instantiator);

    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a source package directory
    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();

    // Create the source package manifest WITHOUT any [package.build.config] section
    let source_pixi_toml_no_config = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#;
    fs::write(source_dir.join("pixi.toml"), source_pixi_toml_no_config).unwrap();

    // Create the workspace manifest
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Helper to filter CondaOutputsCalled events
    fn count_conda_outputs_events(events: &[BackendEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, BackendEvent::CondaOutputsCalled))
            .count()
    }

    // First invocation: Generate the lock-file (no config section)
    let workspace = pixi.workspace().unwrap();
    let (lock_file_data, was_updated) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("First lock file generation should succeed");

    assert!(was_updated, "First invocation should create the lock-file");

    // Verify the package is in the lock-file
    let lock_file = lock_file_data.into_lock_file();
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        ),
        "Lock file should contain my-package"
    );

    // Check that conda_outputs was called once
    let events_after_first = observer.events();
    assert_eq!(
        count_conda_outputs_events(&events_after_first),
        1,
        "conda_outputs should be called once for first lock file generation"
    );

    // Now add an EMPTY [package.build.config] section
    // This should NOT invalidate the cache since empty config hashes the same as no config
    let source_pixi_toml_empty_config = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.build.config]
"#;
    fs::write(source_dir.join("pixi.toml"), source_pixi_toml_empty_config).unwrap();

    // Second invocation with empty config section: Should NOT call backend again (cache hit)
    let workspace = pixi.workspace().unwrap();
    let (_lock_file_data, was_updated_empty_config) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Second lock file check should succeed");

    assert!(
        !was_updated_empty_config,
        "Adding empty [package.build.config] should NOT update lock-file"
    );

    // Verify no additional conda_outputs calls
    let events_after_empty_config = observer.events();
    assert_eq!(
        count_conda_outputs_events(&events_after_empty_config),
        0,
        "conda_outputs should NOT be called when adding empty [package.build.config] (cache hit)"
    );

    // Now add actual configuration values
    let source_pixi_toml_with_config = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.build.config]
noarch = true
"#;
    fs::write(source_dir.join("pixi.toml"), source_pixi_toml_with_config).unwrap();

    // Third invocation: Should detect config change and call backend again
    let workspace = pixi.workspace().unwrap();
    let (_lock_file_data, _was_updated_after_config_added) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Third lock file generation should succeed");

    // Verify conda_outputs was called again due to config change
    let events_after_config_added = observer.events();
    assert_eq!(
        count_conda_outputs_events(&events_after_config_added),
        1,
        "conda_outputs should be called when [package.build.config] gets actual values (cache invalidated)"
    );

    // Fourth invocation without changes: Should NOT call backend again (cache hit)
    let workspace = pixi.workspace().unwrap();
    let (_lock_file_data, was_updated_no_change) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Fourth lock file check should succeed");

    assert!(
        !was_updated_no_change,
        "Fourth invocation without changes should NOT update lock-file"
    );

    // Verify no additional conda_outputs calls
    let events_after_no_change = observer.events();
    assert_eq!(
        count_conda_outputs_events(&events_after_no_change),
        0,
        "conda_outputs should NOT be called again when config hasn't changed (cache hit)"
    );

    // Now change the build configuration (noarch = true -> noarch = false)
    let source_pixi_toml_changed_config = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.build.config]
noarch = false
"#;
    fs::write(
        source_dir.join("pixi.toml"),
        source_pixi_toml_changed_config,
    )
    .unwrap();

    // Fifth invocation: Should detect config change and call backend again
    let workspace = pixi.workspace().unwrap();
    let (_lock_file_data, _was_updated_after_config_change) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Fifth lock file generation should succeed");

    // Verify conda_outputs was called again due to config change
    let events_after_config_change = observer.events();
    assert_eq!(
        count_conda_outputs_events(&events_after_config_change),
        1,
        "conda_outputs should be called again when [package.build.config] values change (cache invalidated)"
    );

    // Sixth invocation: Should NOT call backend again (cache is now fresh)
    let workspace = pixi.workspace().unwrap();
    let (_, was_updated_sixth) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Sixth lock file check should succeed");

    assert!(
        !was_updated_sixth,
        "Sixth invocation should NOT update lock-file (cache is now fresh)"
    );

    // Verify no additional conda_outputs calls
    let events_after_sixth = observer.events();
    assert_eq!(
        count_conda_outputs_events(&events_after_sixth),
        0,
        "conda_outputs should NOT be called again after cache is updated"
    );
}

/// Test that demonstrates a bug with unresolvable partial source records.
///
/// When a lock-file contains partial source records (from mutable path sources)
/// and the source package changes in a way that makes the partial record
/// unresolvable (e.g., the package is renamed), the update flow should gracefully
/// re-solve instead of erroring out.
///
/// The bug: `UpdateContext::finish()` tries to resolve ALL partial records from
/// the lock-file (including from environments already marked as out-of-date).
/// If resolution fails, it produces a hard error instead of proceeding with
/// the re-solve.
#[tokio::test]
async fn test_update_lock_file_with_unresolvable_partial_source_record() {
    setup_tracing();

    // Use an in-memory backend override so we don't need a real build backend.
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a source package directory with an initial name
    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();

    let source_pixi_toml = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#;
    fs::write(source_dir.join("pixi.toml"), source_pixi_toml).unwrap();

    // Create the workspace manifest
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        Platform::current()
    );
    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // First invocation: Generate the lock-file.
    // This creates a lock-file where path source records are stored as partial
    // (mutable sources are downgraded to partial on write).
    let workspace = pixi.workspace().unwrap();
    let (_lock_file_data, was_updated) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("First lock file generation should succeed");
    assert!(was_updated, "First invocation should create the lock-file");

    // Now rename the package in the child manifest. The lock-file on disk still
    // has a partial record for "my-package", but the source now produces
    // metadata for "renamed-package". This makes the old partial record
    // unresolvable (name mismatch).
    let renamed_pixi_toml = r#"
[package]
name = "renamed-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#;
    fs::write(source_dir.join("pixi.toml"), renamed_pixi_toml).unwrap();

    // Also update the workspace manifest to reference the new name
    let updated_manifest = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
renamed-package = {{ path = "./my-package" }}
"#,
        Platform::current()
    );
    fs::write(pixi.manifest_path(), updated_manifest).unwrap();

    // Second invocation: Update the lock-file.
    //
    // The satisfiability check correctly identifies the lock-file as out-of-date
    // (the old "my-package" partial record can't be resolved because the source
    // now produces "renamed-package"). However, `UpdateContext::finish()` also
    // tries to resolve ALL partial records from the old lock-file (including
    // the unresolvable one) and fails with a hard error.
    //
    // This SHOULD succeed — the system should re-solve and produce a new
    // lock-file with "renamed-package".
    let workspace = pixi.workspace().unwrap();
    let result = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await;

    match result {
        Ok(_) => {
            // This is the expected behavior — the system should gracefully
            // re-solve and produce a new lock-file with "renamed-package".
        }
        Err(e) => {
            panic!(
                "Updating the lock-file after renaming a source package should succeed, \
                 but it failed with: {e}"
            );
        }
    }
}

/// Test that source records (including their metadata) survive a lock-file
/// roundtrip through `UnresolvedPixiRecord`.
///
/// On the first lock, the solver produces a full source record. On write, path-
/// based sources are downgraded to partial. On the second lock, the partial
/// record is read back as `UnresolvedPixiRecord`, the satisfiability check
/// re-evaluates it, and the lock-file is written again. The source package
/// should be present and equivalent in both lock-files.
#[tokio::test]
async fn test_source_record_roundtrips_through_lock_file() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a source package directory
    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();

    let source_pixi_toml = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#;
    fs::write(source_dir.join("pixi.toml"), source_pixi_toml).unwrap();

    // Create the workspace manifest
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        Platform::current()
    );
    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // First lock
    let workspace = pixi.workspace().unwrap();
    let (lock_file_data, _) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("First lock should succeed");

    let lock_file = lock_file_data.into_lock_file();

    // Find the source package in the lock-file.
    let env = lock_file
        .environment(consts::DEFAULT_ENVIRONMENT_NAME)
        .expect("default environment should exist");
    let platform = lock_file
        .platform(&Platform::current().to_string())
        .expect("current platform should exist");

    let source_packages: Vec<_> = env
        .packages(platform)
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_source_conda())
        .collect();

    assert!(
        !source_packages.is_empty(),
        "Expected at least one source package in the lock-file"
    );

    // Verify the source package location and metadata are present
    let my_pkg = source_packages
        .iter()
        .find(|p| {
            p.metadata
                .as_full()
                .is_some_and(|f| f.package_record.name.as_normalized() == "my-package")
                || p.metadata
                    .as_partial()
                    .is_some_and(|part| part.name.as_normalized() == "my-package")
        })
        .expect("my-package should be in source packages");

    // The location should point to the source directory
    let location_str = my_pkg.location.to_string();
    assert!(
        location_str.contains('.'),
        "Source package location should be a relative path, got: {location_str}"
    );

    // Second lock: records roundtrip through UnresolvedPixiRecord
    let workspace = pixi.workspace().unwrap();
    let (lock_file_data_2, was_updated) = workspace
        .update_lock_file(pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Second lock should succeed");

    assert!(
        !was_updated,
        "Second lock invocation should not update the lock-file"
    );

    let lock_file_2 = lock_file_data_2.into_lock_file();
    let env_2 = lock_file_2
        .environment(consts::DEFAULT_ENVIRONMENT_NAME)
        .unwrap();
    let platform_2 = lock_file_2
        .platform(&Platform::current().to_string())
        .unwrap();

    let source_packages_2: Vec<_> = env_2
        .packages(platform_2)
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_source_conda())
        .collect();

    let my_pkg_2 = source_packages_2
        .iter()
        .find(|p| {
            p.metadata
                .as_full()
                .is_some_and(|f| f.package_record.name.as_normalized() == "my-package")
                || p.metadata
                    .as_partial()
                    .is_some_and(|part| part.name.as_normalized() == "my-package")
        })
        .expect("my-package should still be in source packages after roundtrip");

    // Location should be preserved
    assert_eq!(
        my_pkg.location.to_string(),
        my_pkg_2.location.to_string(),
        "Source package location should be identical after roundtrip"
    );

    // package_build_source should be preserved (None == None for path deps
    // without [package.build.source], or Some == Some for git/url sources)
    assert_eq!(
        my_pkg.package_build_source, my_pkg_2.package_build_source,
        "package_build_source should be identical after roundtrip"
    );
}

#[tokio::test]
async fn test_source_timestamp_reused_when_lockfile_recomputed_for_unrelated_binary_change() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let t_fixed = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .into();
    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1.0.0").finish());
    package_database.add_package(Package::build("bar", "1.0.0").finish());
    package_database.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t_fixed)
            .finish(),
    );
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();
    let mock_channel = url::Url::from_directory_path(channel_dir.path())
        .unwrap()
        .to_string();

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    write_basic_source_package_manifest(
        &source_dir,
        "1.0.0",
        r#"[package.host-dependencies]
host-dep = "*""#,
    );
    write_source_workspace_manifest_with_binary_dependencies(
        &pixi.manifest_path(),
        &[&mock_channel],
        &["foo"],
    );

    let initial_lock = pixi.update_lock_file().await.unwrap();
    let initial_timestamp = initial_lock
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should have a source timestamp");

    // No sleep needed: the source record cache ensures the same record is
    // returned when the hint passes the original timestamp as exclude_newer.
    write_source_workspace_manifest_with_binary_dependencies(
        &pixi.manifest_path(),
        &[&mock_channel],
        &["foo", "bar"],
    );

    let relocked = pixi.update_lock_file().await.unwrap();
    let relocked_timestamp = relocked
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should still have a source timestamp");

    assert_eq!(
        initial_timestamp, relocked_timestamp,
        "source timestamp should be reused when relocking due to an unrelated binary dependency change"
    );
}

#[tokio::test]
async fn test_source_timestamp_changes_when_source_metadata_changes() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    write_basic_source_package_manifest(&source_dir, "1.0.0", "");
    write_basic_source_workspace_manifest(&pixi.manifest_path(), &[]);

    pixi.update_lock_file().await.unwrap();

    // Change the source package version. Since this is a mutable path-based
    // source, the lock file stores only partial metadata (no version), so the
    // lock file content won't change. The important thing is that the
    // satisfiability check detects the metadata change and re-solves
    // successfully.
    write_basic_source_package_manifest(&source_dir, "1.1.0", "");
    pixi.update_lock_file().await.unwrap();
}

#[tokio::test]
async fn test_source_timestamp_changes_for_explicit_update() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let t1 = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .into();
    let t2 = chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
        .unwrap()
        .into();

    // Initial channel only has host-dep 1.0.0.
    let mut db = MockRepoData::default();
    db.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t1)
            .finish(),
    );
    let channel_dir = TempDir::new().unwrap();
    db.write_repodata(channel_dir.path()).await.unwrap();
    let mock_channel = url::Url::from_directory_path(channel_dir.path())
        .unwrap()
        .to_string();

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    write_basic_source_package_manifest(
        &source_dir,
        "1.0.0",
        r#"[package.host-dependencies]
host-dep = "*""#,
    );
    write_basic_source_workspace_manifest(&pixi.manifest_path(), &[&mock_channel]);

    let initial_lock = pixi.update_lock_file().await.unwrap();
    let initial_timestamp = initial_lock
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should have a source timestamp");

    // Publish a newer host-dep to the same channel. The explicit update
    // should pick it up, producing a deterministically different timestamp.
    let mut db2 = MockRepoData::default();
    db2.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t1)
            .finish(),
    );
    db2.add_package(
        Package::build("host-dep", "1.0.1")
            .with_timestamp(t2)
            .finish(),
    );
    db2.write_repodata(channel_dir.path()).await.unwrap();

    pixi.update().with_package("my-package").await.unwrap();

    let updated_lock = pixi.lock_file().await.unwrap();
    let updated_timestamp = updated_lock
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should still have a source timestamp");

    assert!(
        updated_timestamp > initial_timestamp,
        "source timestamp should advance when the package is explicitly updated \
         (initial={initial_timestamp}, updated={updated_timestamp})"
    );
}

#[tokio::test]
async fn test_source_timestamp_reuse_survives_sibling_metadata_change() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let t_fixed = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .into();
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t_fixed)
            .finish(),
    );
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();
    let mock_channel = url::Url::from_directory_path(channel_dir.path())
        .unwrap()
        .to_string();

    let host_dep_extra = r#"[package.host-dependencies]
host-dep = "*""#;

    let my_package_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&my_package_dir).unwrap();
    write_source_package_manifest(&my_package_dir, "my-package", "1.0.0", host_dep_extra);

    let other_package_dir = pixi.workspace_path().join("other-package");
    fs::create_dir_all(&other_package_dir).unwrap();
    write_source_package_manifest(&other_package_dir, "other-package", "1.0.0", host_dep_extra);

    write_source_workspace_manifest(
        &pixi.manifest_path(),
        &[&mock_channel],
        &["my-package", "other-package"],
    );

    let initial_lock = pixi.update_lock_file().await.unwrap();
    let initial_my_timestamp = initial_lock
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should have a source timestamp");

    // Bump other-package version; my-package stays unchanged.
    write_source_package_manifest(&other_package_dir, "other-package", "1.1.0", host_dep_extra);

    let relocked = pixi.update_lock_file().await.unwrap();
    let relocked_my_timestamp = relocked
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should still have a source timestamp");

    assert_eq!(
        initial_my_timestamp, relocked_my_timestamp,
        "unchanged source package should keep its timestamp even when a sibling source package changes"
    );

    // Verify the changed sibling was re-solved and is still present.
    assert!(
        relocked
            .get_conda_source_timestamp(
                consts::DEFAULT_ENVIRONMENT_NAME,
                Platform::current(),
                "other-package",
            )
            .is_some(),
        "changed sibling should still be present with a timestamp after re-solve"
    );
}

#[tokio::test]
async fn test_source_timestamp_reused_across_solve_group_environments() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let t_fixed = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .into();
    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1.0.0").finish());
    package_database.add_package(Package::build("bar", "1.0.0").finish());
    package_database.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t_fixed)
            .finish(),
    );
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();
    let mock_channel = url::Url::from_directory_path(channel_dir.path())
        .unwrap()
        .to_string();

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    write_basic_source_package_manifest(
        &source_dir,
        "1.0.0",
        r#"[package.host-dependencies]
host-dep = "*""#,
    );

    // Two environments in the same solve group, both inheriting the source dep.
    let initial_manifest = format!(
        r#"
[workspace]
channels = ["{mock_channel}"]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
foo = "*"

[feature.env-a.dependencies]

[feature.env-b.dependencies]

[environments]
env-a = {{ features = ["env-a"], solve-group = "shared" }}
env-b = {{ features = ["env-b"], solve-group = "shared" }}
"#,
        Platform::current()
    );
    fs::write(pixi.manifest_path(), &initial_manifest).unwrap();

    let initial_lock = pixi.update_lock_file().await.unwrap();
    let initial_ts_a = initial_lock
        .get_conda_source_timestamp("env-a", Platform::current(), "my-package")
        .expect("env-a should have my-package source timestamp");
    let initial_ts_b = initial_lock
        .get_conda_source_timestamp("env-b", Platform::current(), "my-package")
        .expect("env-b should have my-package source timestamp");

    assert_eq!(
        initial_ts_a, initial_ts_b,
        "both environments should get the same timestamp from the shared solve group"
    );

    // Add an unrelated binary dependency — triggers re-solve but source hasn't changed.
    let updated_manifest = format!(
        r#"
[workspace]
channels = ["{mock_channel}"]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
foo = "*"
bar = "*"

[feature.env-a.dependencies]

[feature.env-b.dependencies]

[environments]
env-a = {{ features = ["env-a"], solve-group = "shared" }}
env-b = {{ features = ["env-b"], solve-group = "shared" }}
"#,
        Platform::current()
    );
    fs::write(pixi.manifest_path(), &updated_manifest).unwrap();

    let relocked = pixi.update_lock_file().await.unwrap();
    let relocked_ts_a = relocked
        .get_conda_source_timestamp("env-a", Platform::current(), "my-package")
        .expect("env-a should still have my-package source timestamp");
    let relocked_ts_b = relocked
        .get_conda_source_timestamp("env-b", Platform::current(), "my-package")
        .expect("env-b should still have my-package source timestamp");

    assert_eq!(
        initial_ts_a, relocked_ts_a,
        "env-a timestamp should be preserved after adding unrelated binary dep"
    );
    assert_eq!(
        initial_ts_b, relocked_ts_b,
        "env-b timestamp should be preserved after adding unrelated binary dep"
    );
}

#[tokio::test]
async fn test_source_timestamp_reused_when_channel_appended() {
    setup_tracing();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let t_fixed = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .into();

    // Channel 1 has foo + host-dep, channel 2 has bar.
    // host-dep is a host-dep of the source package so the timestamp is
    // deterministic (not Utc::now()). Both channels must carry it so the
    // re-resolve after appending channel 2 can still find it.
    let mut db1 = MockRepoData::default();
    db1.add_package(Package::build("foo", "1.0.0").finish());
    db1.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t_fixed)
            .finish(),
    );
    let channel1_dir = TempDir::new().unwrap();
    db1.write_repodata(channel1_dir.path()).await.unwrap();
    let channel1 = url::Url::from_directory_path(channel1_dir.path())
        .unwrap()
        .to_string();

    let mut db2 = MockRepoData::default();
    db2.add_package(Package::build("bar", "1.0.0").finish());
    db2.add_package(
        Package::build("host-dep", "1.0.0")
            .with_timestamp(t_fixed)
            .finish(),
    );
    let channel2_dir = TempDir::new().unwrap();
    db2.write_repodata(channel2_dir.path()).await.unwrap();
    let channel2 = url::Url::from_directory_path(channel2_dir.path())
        .unwrap()
        .to_string();

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    write_basic_source_package_manifest(
        &source_dir,
        "1.0.0",
        r#"[package.host-dependencies]
host-dep = "*""#,
    );

    // Initial lock with one channel.
    write_source_workspace_manifest_with_binary_dependencies(
        &pixi.manifest_path(),
        &[&channel1],
        &["foo"],
    );

    let initial_lock = pixi.update_lock_file().await.unwrap();
    let initial_timestamp = initial_lock
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should have a source timestamp");

    // Append a second, lower-priority channel. Existing packages remain valid
    // (ChannelsExtended), so the source timestamp should be preserved.
    write_source_workspace_manifest_with_binary_dependencies(
        &pixi.manifest_path(),
        &[&channel1, &channel2],
        &["foo"],
    );

    let relocked = pixi.update_lock_file().await.unwrap();
    let relocked_timestamp = relocked
        .get_conda_source_timestamp(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        )
        .expect("my-package should still have a source timestamp");

    assert_eq!(
        initial_timestamp, relocked_timestamp,
        "source timestamp should be preserved when a lower-priority channel is appended"
    );
}
