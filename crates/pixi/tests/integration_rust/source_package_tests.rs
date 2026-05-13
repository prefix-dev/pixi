use fs_err as fs;
use pixi_build_backend_passthrough::{BackendEvent, ObservableBackend, PassthroughBackend};
use pixi_build_frontend::BackendOverride;
use pixi_consts::consts;
use rattler_conda_types::{Platform, package::RunExportsJson};
use rattler_lock::{LockFile, PackageBuildSource};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use url::Url;

use crate::{
    common::{LockFileExt, PixiControl},
    setup_tracing,
};
use pixi_cli::publish;
use pixi_test_utils::{GitRepoFixture, MockRepoData, Package, format_diagnostic};

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

/// Verifies that the workspace exclude-newer cutoff propagates into
/// the source package's build-dependency solve during lockfile
/// update.
///
/// Currently ignored: with the SourceBuildKey migration, nested
/// build/host solves are expected to happen upstream in the
/// orchestrator (ResolveSourcePackageKey → SolvePixiEnvironmentKey),
/// but the PassthroughBackend fixture produces a lockfile where
/// build_packages stays empty even though the package manifest lists
/// a build-dependency on `foo`. Re-enable once the orchestrator-side
/// nested-solve path has been audited end-to-end for exclude_newer
/// propagation.
#[tokio::test]
#[ignore = "nested build-dep solve for source packages is not yet verified through the orchestrator path"]
async fn test_source_dependency_inherits_exclude_newer_for_build_dependencies_during_lock_update() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("foo", "1")
            .with_timestamp("2026-01-10T00:00:00Z".parse().unwrap())
            .with_materialize(true)
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ));

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    fs::write(
        source_dir.join("pixi.toml"),
        r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.build-dependencies]
foo = "*"
"#,
    )
    .unwrap();

    let manifest_without_cutoff = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel = channel.url(),
        platform = Platform::current(),
    );
    pixi.update_manifest(&manifest_without_cutoff).unwrap();
    pixi.update_lock_file()
        .await
        .expect("source dependency should lock without exclude-newer");

    let manifest_with_cutoff = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]
exclude-newer = "2025-01-01T00:00:00Z"

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel = channel.url(),
        platform = Platform::current(),
    );
    pixi.update_manifest(&manifest_with_cutoff).unwrap();

    let err = pixi
        .update_lock_file()
        .await
        .expect_err("source build env solve should inherit exclude-newer during lock update");
    let rendered = format_diagnostic(err.as_ref());
    assert!(
        rendered.contains("failed to solve the environment"),
        "{rendered}"
    );
    assert!(rendered.contains("foo"), "{rendered}");
}

fn variant_fail_fast_manifest(channel: &str, platform: Platform) -> String {
    format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[workspace.build-variants]
sdl2 = ["2.26.5", "2.32.*"]

[package]
name = "variant-fail-fast"
version = "1.0.0"

[package.build]
backend = {{ name = "in-memory", version = "0.1.0" }}

[package.host-dependencies]
sdl2 = "*"
"#,
    )
}

#[tokio::test]
async fn test_publish_fails_before_build_or_upload_when_one_variant_is_unsatisfiable() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("sdl2", "2.26.5")
            .with_materialize(true)
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let (instantiator, mut observer) =
        ObservableBackend::instantiator(PassthroughBackend::instantiator());
    let pixi = PixiControl::from_manifest(&variant_fail_fast_manifest(
        channel.url().as_ref(),
        Platform::current(),
    ))
    .unwrap();

    let publish_dir = tempfile::tempdir().unwrap();
    let target_url = Url::from_directory_path(publish_dir.path()).unwrap();
    let err = publish::execute(publish::Args {
        backend_override: Some(BackendOverride::from_memory(instantiator)),
        config_cli: Default::default(),
        target_platform: Platform::current(),
        build_platform: Platform::current(),
        build_string_prefix: None,
        build_number: None,
        build_dir: None,
        clean: false,
        path: Some(pixi.manifest_path()),
        target_channel: Some(target_url.to_string()),
        target_dir: None,
        force: false,
        skip_existing: true,
        generate_attestation: false,
        variant: Vec::new(),
        variant_config: Vec::new(),
    })
    .await
    .expect_err("publish should fail when one variant cannot be resolved");

    let rendered = format_diagnostic(err.as_ref());
    assert!(
        rendered.contains("solve the host environment"),
        "{rendered}"
    );
    assert!(rendered.contains("sdl2"), "{rendered}");
    assert!(
        observer.build_events().is_empty(),
        "publish should fail during pre-resolution before any build starts"
    );
    let published_artifacts = fs::read_dir(publish_dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .count();
    assert_eq!(
        published_artifacts, 0,
        "publish should not upload any artifacts"
    );
}

#[tokio::test]
async fn test_source_dependency_honors_exclude_newer_overrides_for_host_and_build_dependencies() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("foo", "1")
            .with_timestamp("2026-01-10T00:00:00Z".parse().unwrap())
            .with_materialize(true)
            .finish(),
    );
    package_database.add_package(
        Package::build("bar", "1")
            .with_timestamp("2026-01-10T00:00:00Z".parse().unwrap())
            .with_materialize(true)
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ));

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    fs::write(
        source_dir.join("pixi.toml"),
        r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.host-dependencies]
bar = "*"

[package.build-dependencies]
foo = "*"
"#,
    )
    .unwrap();

    let cutoff = "2025-01-01T00:00:00Z";
    let override_cutoff = "2026-12-31T00:00:00Z";

    let manifest_with_build_override_only = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]
exclude-newer = "{cutoff}"

[dependencies]
my-package = {{ path = "./my-package" }}

[exclude-newer]
foo = "{override_cutoff}"
"#,
        channel = channel.url(),
        platform = Platform::current(),
        cutoff = cutoff,
        override_cutoff = override_cutoff,
    );
    pixi.update_manifest(&manifest_with_build_override_only)
        .unwrap();

    let err = pixi
        .install()
        .await
        .expect_err("host dependency should still be excluded until it is overridden too");
    let rendered = format_diagnostic(err.as_ref());
    assert!(
        rendered.contains("while trying to solve the host environment"),
        "{rendered}"
    );
    assert!(rendered.contains("bar"), "{rendered}");

    let manifest_with_both_overrides = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]
exclude-newer = "{cutoff}"

[dependencies]
my-package = {{ path = "./my-package" }}

[exclude-newer]
foo = "{override_cutoff}"
bar = "{override_cutoff}"
"#,
        channel = channel.url(),
        platform = Platform::current(),
        cutoff = cutoff,
        override_cutoff = override_cutoff,
    );
    pixi.update_manifest(&manifest_with_both_overrides).unwrap();

    pixi.install()
        .await
        .expect("timestamp-less pixi-build source packages should remain eligible");
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("Second lock file check should succeed");

    // The second invocation should NOT update the lock-file since it's already up to date
    assert!(
        !was_updated_second,
        "Second invocation should report lock-file is already up to date"
    );
}

/// Adding a `[package.run-dependencies]` entry to a path-based source
/// package must invalidate the lock-file on the next resolve.
///
/// The locked source record's `depends` field is the union of the
/// manifest's run-dependencies and any run-exports contributed by the
/// resolved build/host packages, so a "diff manifest run-deps against
/// locked depends" check can't tell which side an entry came from. The
/// only reliable signal is the backend's `run_dependencies`
/// declaration: satisfiability must consult the backend and reject the
/// lock-file when its declarations no longer match what's locked.
///
/// `PassthroughBackend` reads `run-dependencies` straight from the
/// source manifest, so editing the manifest changes what the backend
/// declares on the next satisfiability call. The first solve produces
/// a lock-file with `dep-a` only; after adding `dep-b`, the lock-file
/// must be rewritten.
#[tokio::test]
async fn test_source_run_dependency_addition_invalidates_lock_file() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("dep-a", "1.0.0").finish());
    package_database.add_package(Package::build("dep-b", "1.0.0").finish());
    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    let initial_source_manifest = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.run-dependencies]
dep-a = ">=1.0"
"#;
    fs::write(source_dir.join("pixi.toml"), initial_source_manifest).unwrap();

    let manifest = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel = channel.url(),
        platform = Platform::current(),
    );
    fs::write(pixi.manifest_path(), manifest).unwrap();

    let workspace = pixi.workspace().unwrap();
    let (_, was_updated) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("initial lock-file generation should succeed");
    assert!(was_updated, "initial solve must create the lock-file");

    // Add a new run-dependency. The locked record's `depends` does not
    // contain `dep-b`, so satisfiability must detect the mismatch
    // against the backend's freshly-declared run-dependencies.
    let updated_source_manifest = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.run-dependencies]
dep-a = ">=1.0"
dep-b = ">=1.0"
"#;
    fs::write(source_dir.join("pixi.toml"), updated_source_manifest).unwrap();

    let workspace = pixi.workspace().unwrap();
    let (_, was_updated_after_add) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("second lock-file check should succeed");
    assert!(
        was_updated_after_add,
        "adding a run-dependency to a source package must invalidate the lock-file",
    );
}

/// Removing a `[package.run-dependencies]` entry from a path-based
/// source package must invalidate the lock-file.
///
/// Counterpart to `test_source_run_dependency_addition_invalidates_lock_file`.
/// The "every backend-declared dep is satisfied by the locked record"
/// shape used for build/host verification doesn't catch removals at
/// all: the remaining backend deps are still locked, but the locked
/// record carries an extra `depends` entry the backend no longer
/// declares. Detection has to be bidirectional, which is what makes
/// run-dep removal trickier than addition.
#[tokio::test]
async fn test_source_run_dependency_removal_invalidates_lock_file() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("dep-a", "1.0.0").finish());
    package_database.add_package(Package::build("dep-b", "1.0.0").finish());
    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    let initial_source_manifest = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.run-dependencies]
dep-a = ">=1.0"
dep-b = ">=1.0"
"#;
    fs::write(source_dir.join("pixi.toml"), initial_source_manifest).unwrap();

    let manifest = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel = channel.url(),
        platform = Platform::current(),
    );
    fs::write(pixi.manifest_path(), manifest).unwrap();

    let workspace = pixi.workspace().unwrap();
    let (_, was_updated) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("initial lock-file generation should succeed");
    assert!(was_updated, "initial solve must create the lock-file");

    // Drop `dep-b`. The locked record still carries it in `depends`,
    // but the backend no longer declares it, and the lock-file must be
    // rewritten so the resolved environment shrinks accordingly.
    let updated_source_manifest = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.run-dependencies]
dep-a = ">=1.0"
"#;
    fs::write(source_dir.join("pixi.toml"), updated_source_manifest).unwrap();

    let workspace = pixi.workspace().unwrap();
    let (_, was_updated_after_remove) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("second lock-file check should succeed");
    assert!(
        was_updated_after_remove,
        "removing a run-dependency from a source package must invalidate the lock-file",
    );
}

/// Removing a host-dependency that contributes a `weak_constrains`
/// run-export must invalidate the lock-file, even though pixi's manifest
/// schema doesn't currently expose `[package.run-constraints]` directly.
///
/// The locked source record's `constrains` field is the union of any
/// manifest-declared run-constraints (none today) and `weak_constrains`
/// contributed by host packages' run-exports. Dropping the host-dep
/// removes the run-export contribution, so the freshly-derived expected
/// `constrains` shrinks to empty while the locked record still carries
/// the run-export-derived spec. The drift detector must catch that.
///
/// This is the integration mirror of the unit-level
/// `verify_locked_run_deps_detects_constrain_removal` test: same shape
/// of drift, but driven through the real backend / build / lockfile
/// pipeline instead of synthesised inputs.
#[tokio::test]
async fn test_host_run_export_constraint_removal_invalidates_lock_file() {
    setup_tracing();

    // Channel hosts `libfoo`; its `weak_constrains` run-export adds
    // `bar <2` to the run-time constraints of any package that has
    // `libfoo` in its host env.
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("libfoo", "1.0.0")
            .with_run_exports(RunExportsJson {
                weak_constrains: vec!["bar <2".to_string()],
                ..Default::default()
            })
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Source package starts with `libfoo` as a host dep so the build
    // pipeline applies its weak_constrains to the built record's
    // `constrains`.
    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    // `noarch = false` opts the passthrough backend out of its
    // NoArch default so the package is platform-specific. Required
    // here because `weak_constrains` run-exports are only applied to
    // non-NoArch packages: a NoArch built with `libfoo` in host-deps
    // would never pick up `bar <2`, leaving nothing for the drift
    // detector to catch.
    let initial_source_manifest = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.build.config]
noarch = false

[package.host-dependencies]
libfoo = "*"
"#;
    fs::write(source_dir.join("pixi.toml"), initial_source_manifest).unwrap();

    let manifest = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel = channel.url(),
        platform = Platform::current(),
    );
    fs::write(pixi.manifest_path(), manifest).unwrap();

    let workspace = pixi.workspace().unwrap();
    let (_, was_updated) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("initial lock-file generation should succeed");
    assert!(was_updated, "initial solve must create the lock-file");

    // Drop the host-dependency. The backend now declares no host deps,
    // so no run-export contributes to the built record's `constrains`,
    // but the locked record still carries `bar <2` from the previous
    // solve.
    let updated_source_manifest = r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "passthrough", version = "*" }

[package.build.config]
noarch = false
"#;
    fs::write(source_dir.join("pixi.toml"), updated_source_manifest).unwrap();

    let workspace = pixi.workspace().unwrap();
    let (_, was_updated_after_drop) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("second lock-file check should succeed");
    assert!(
        was_updated_after_drop,
        "removing a host-dep that contributed a weak_constrains run-export must invalidate the lock-file",
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
                .is_some_and(|package_record| package_record.name.as_normalized() == "my-package")
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
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
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
                .is_some_and(|package_record| package_record.name.as_normalized() == "my-package")
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

/// A re-lock that's triggered by a source's host/build-deps changing
/// must keep already-locked build/host package versions when they
/// still satisfy the new specs. This is the "incremental update"
/// guarantee: the solver receives the locked build/host package set
/// as installed hints (via `installed_source_hints`), so it prefers
/// those versions over picking the latest available.
///
/// Without that hint, the new solve would pick `foo 2.0` (the highest
/// version in the channel). With it, the locked `foo 1.0` is kept and
/// only the newly-required `bar` is solved fresh.
#[tokio::test]
async fn test_relock_keeps_locked_build_packages_as_installed_hints() {
    setup_tracing();

    // Channel hosts two versions of `foo` plus one `bar`. The version
    // gap on `foo` is what makes the test discriminate "preserved" from
    // "re-resolved": with no installed hint a fresh solve would pick
    // `foo 2.0`.
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("foo", "1.0.0")
            .with_timestamp("2025-01-01T00:00:00Z".parse().unwrap())
            .with_materialize(true)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "2.0.0")
            .with_timestamp("2025-06-01T00:00:00Z".parse().unwrap())
            .with_materialize(true)
            .finish(),
    );
    package_database.add_package(
        Package::build("bar", "1.0.0")
            .with_timestamp("2025-01-01T00:00:00Z".parse().unwrap())
            .with_materialize(true)
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ));

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();

    // Initial source manifest: pin foo to 1.0 so the first solve can
    // only pick that exact version, regardless of channel content.
    fs::write(
        source_dir.join("pixi.toml"),
        r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.build-dependencies]
foo = "==1.0.0"
"#,
    )
    .unwrap();

    let workspace_manifest = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
        channel = channel.url(),
        platform = Platform::current(),
    );
    pixi.update_manifest(&workspace_manifest).unwrap();

    // First solve. The build env should be locked with foo 1.0.
    let workspace = pixi.workspace().unwrap();
    let (lock_data, _) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("first solve should succeed");
    let lock_v1 = lock_data.into_lock_file();
    let foo_versions_v1 = collect_build_dep_versions(&lock_v1, "my-package", "foo");
    assert_eq!(
        foo_versions_v1,
        vec!["1.0.0"],
        "first solve must pin foo to 1.0.0; got {foo_versions_v1:?}"
    );

    // Now the source loosens its constraint on `foo` (any version is
    // allowed) and adds a new build-dep `bar`. The added bar means
    // the locked build_packages are no longer enough to satisfy the
    // backend's specs, so satisfiability triggers a re-lock. The
    // re-lock receives the locked `foo 1.0.0` as an installed hint;
    // the solver should keep it instead of jumping to `foo 2.0.0`.
    fs::write(
        source_dir.join("pixi.toml"),
        r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.build-dependencies]
foo = "*"
bar = "*"
"#,
    )
    .unwrap();

    let workspace = pixi.workspace().unwrap();
    let (lock_data, was_updated) = workspace
        .update_lock_file(None, pixi_core::UpdateLockFileOptions::default())
        .await
        .expect("second solve should succeed and re-lock");
    assert!(
        was_updated,
        "second solve must re-lock since build-dependencies changed"
    );
    let lock_v2 = lock_data.into_lock_file();

    let foo_versions_v2 = collect_build_dep_versions(&lock_v2, "my-package", "foo");
    assert_eq!(
        foo_versions_v2,
        vec!["1.0.0"],
        "re-lock must keep the locked foo 1.0.0 (installed hint), not jump to 2.0.0; got {foo_versions_v2:?}"
    );
    let bar_versions_v2 = collect_build_dep_versions(&lock_v2, "my-package", "bar");
    assert_eq!(
        bar_versions_v2,
        vec!["1.0.0"],
        "re-lock must add bar 1.0.0 to the build env; got {bar_versions_v2:?}"
    );
}

/// Collect the versions of a binary package that appears in
/// `source_pkg`'s `build_packages` slot, across every platform.
/// Used by the incremental-relock tests to check what survived a
/// re-lock.
fn collect_build_dep_versions(
    lock_file: &rattler_lock::LockFile,
    source_pkg: &str,
    binary_dep: &str,
) -> Vec<String> {
    collect_source_dep_versions(lock_file, source_pkg, binary_dep, SourceDepSlot::Build)
}

/// Collect the versions of a binary package that appears in
/// `source_pkg`'s `host_packages` slot, across every platform.
fn collect_host_dep_versions(
    lock_file: &rattler_lock::LockFile,
    source_pkg: &str,
    binary_dep: &str,
) -> Vec<String> {
    collect_source_dep_versions(lock_file, source_pkg, binary_dep, SourceDepSlot::Host)
}

#[derive(Clone, Copy)]
enum SourceDepSlot {
    Build,
    Host,
}

fn collect_source_dep_versions(
    lock_file: &rattler_lock::LockFile,
    source_pkg: &str,
    binary_dep: &str,
    slot: SourceDepSlot,
) -> Vec<String> {
    use pixi_record::LockFileResolver;
    use std::path::Path;

    let resolver =
        LockFileResolver::build(lock_file, Path::new("/")).expect("lockfile must resolve cleanly");
    let mut out = Vec::new();
    for (_env_name, env) in lock_file.environments() {
        for (_platform, packages) in env.packages_by_platform() {
            for package in packages {
                let Some(record) = resolver.get_for_package(package) else {
                    continue;
                };
                let pixi_record::UnresolvedPixiRecord::Source(src) = record else {
                    continue;
                };
                if src.name().as_normalized() != source_pkg {
                    continue;
                }
                let slot_packages = match slot {
                    SourceDepSlot::Build => &src.build_packages,
                    SourceDepSlot::Host => &src.host_packages,
                };
                for slot_pkg in slot_packages {
                    if let pixi_record::UnresolvedPixiRecord::Binary(b) = slot_pkg
                        && b.package_record.name.as_normalized() == binary_dep
                    {
                        out.push(b.package_record.version.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out
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

/// `pixi update sdl2` must invalidate `sdl2` everywhere it appears in
/// the lockfile, including inside source records' `host_packages`
/// arrays. Today only the top-level locked package is relaxed; the
/// stale copy of `sdl2` inside `my-package.host_packages` survives
/// the relaxation pass, leaving the lockfile in an inconsistent
/// state.
///
/// Setup: `sdl2` v2.26.5 is the only version in the channel; the
/// workspace depends on a path-based source package `my-package`
/// whose `host-dependencies` include `sdl2`, plus a top-level
/// `sdl2 = "*"` so the user can target it by name. After the first
/// lock, `sdl2 2.26.5` lands in both the top-level env and inside
/// `my-package.host_packages`. We publish `sdl2 2.32.0` and run
/// `pixi update sdl2`.
///
/// Today this test fails: the re-lock errors out with a build/host
/// graph cycle ("my-package -> my-package") because the relaxation
/// stripped sdl2 from the top level but kept it inside the source
/// record, so the resolver walks an inconsistent graph. Stripping
/// update targets out of source records' build/host arrays during
/// relaxation removes the inconsistency: the re-solve picks
/// `sdl2 2.32.0` cleanly in both slots and the post-update
/// assertions below pass.
#[tokio::test]
async fn test_update_invalidates_transitive_in_source_host_packages() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("sdl2", "2.26.5").finish());
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        ));

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();
    fs::write(
        source_dir.join("pixi.toml"),
        r#"
[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }

[package.host-dependencies]
sdl2 = "*"
"#,
    )
    .unwrap();

    let channel_url = Url::from_file_path(channel_dir.path()).unwrap();
    let workspace_manifest = format!(
        r#"
[workspace]
channels = ["{channel}"]
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
sdl2 = "*"
"#,
        channel = channel_url,
        platform = Platform::current(),
    );
    pixi.update_manifest(&workspace_manifest).unwrap();

    // First lock pins sdl2 2.26.5 both at the top level and inside
    // my-package.host_packages — confirms the precondition the bug
    // depends on.
    let lock_v1 = pixi
        .update_lock_file()
        .await
        .expect("first lock should succeed");
    assert!(
        lock_v1.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "sdl2 ==2.26.5",
        ),
        "first lock must pin top-level sdl2 to 2.26.5"
    );
    assert_eq!(
        collect_host_dep_versions(&lock_v1, "my-package", "sdl2"),
        vec!["2.26.5"],
        "first lock must pin my-package.host_packages sdl2 to 2.26.5"
    );

    // Publish a newer sdl2.
    package_database.add_package(Package::build("sdl2", "2.32.0").finish());
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Run the actual CLI update path: this exercises `unlock_packages`
    // → `UpdateContext` end-to-end, which is where the bug lives.
    pixi.update()
        .with_package("sdl2")
        .await
        .expect("update sdl2 should succeed");

    let lock_v2 = pixi.lock_file().await.unwrap();
    assert!(
        lock_v2.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "sdl2 ==2.32.0",
        ),
        "top-level sdl2 must be updated to 2.32.0"
    );
    assert_eq!(
        collect_host_dep_versions(&lock_v2, "my-package", "sdl2"),
        vec!["2.32.0"],
        "my-package.host_packages must also be updated to sdl2 2.32.0; \
         a stale 2.26.5 here means `pixi update sdl2` left a transitive \
         copy untouched"
    );
}

/// Sorted list of the git-typed `package_build_source` entries on every
/// source record in the lock file. Mirrors the Python helper: it's the
/// signal that flips when the workspace's git pin changes.
fn extract_git_build_sources(lock_file: &LockFile) -> Vec<PackageBuildSource> {
    let mut out = Vec::new();
    for (_, env) in lock_file.environments() {
        for (_, packages) in env.packages_by_platform() {
            for pkg in packages {
                let Some(src) = pkg.as_source_conda() else {
                    continue;
                };
                if let Some(build_source @ PackageBuildSource::Git { .. }) =
                    src.package_build_source.clone()
                {
                    out.push(build_source);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `pixi install --locked` must reject a manifest whose git ref no longer
/// matches the lock without rewriting the lock, and a regular `pixi lock`
/// must update the lock to the new ref. Mirrors the (now-removed) python
/// `test_git_path_lock_behaviour`.
#[tokio::test]
async fn test_git_path_lock_behaviour() {
    setup_tracing();

    // Build a repo with `main` and `other-feature` branches at distinct
    // revs. The `lock-behaviour-base` fixture ships the initial commit;
    // the rest is layered on through the GitRepoFixture::git escape
    // hatch since the numbered-fixture format can't express branches.
    let fixture = GitRepoFixture::new("lock-behaviour-base");
    fixture.git(&["checkout", "-b", "other-feature"]);
    fs::write(
        fixture.repo_path.join("README.md"),
        "other-feature change\n",
    )
    .unwrap();
    fixture.git(&["commit", "-am", "other-feature change"]);
    let other_feature_rev = fixture.git(&["rev-parse", "HEAD"]);
    fixture.git(&["checkout", "main"]);
    fs::write(fixture.repo_path.join("README.md"), "main update\n").unwrap();
    fixture.git(&["commit", "-am", "main update"]);
    let main_rev = fixture.git(&["rev-parse", "HEAD"]);
    assert_ne!(main_rev, other_feature_rev);

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let manifest_path = pixi.manifest_path();
    let workspace_path: PathBuf = pixi.workspace_path().into();
    let git_url = &fixture.base_url;
    let write_manifest = |kind: &str, value: &str| {
        let manifest = format!(
            r#"
[workspace]
channels = []
platforms = ["{platform}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "." }}

[package]
name = "my-package"
version = "0.1.0"

[package.build]
backend = {{ name = "passthrough", version = "*" }}

[package.build.source]
git = "{git_url}"
subdirectory = "."
{kind} = "{value}"
"#,
            platform = Platform::current(),
        );
        fs::write(&manifest_path, manifest).unwrap();
    };

    // Pin to main_rev and produce the initial lock.
    write_manifest("rev", &main_rev);
    pixi.lock().await.unwrap();
    let initial = extract_git_build_sources(&pixi.lock_file().await.unwrap());
    assert!(
        !initial.is_empty(),
        "expected at least one git package_build_source entry in the lock"
    );

    // `--locked` must accept a manifest that matches the lock and leave
    // the lock byte-identical.
    pixi.install().with_locked().await.unwrap();
    assert_eq!(
        extract_git_build_sources(&pixi.lock_file().await.unwrap()),
        initial,
        "successful --locked install must not rewrite the lock"
    );

    // Swap the manifest to a branch that resolves to a different rev.
    // The lock now lists the old rev → manifest mismatch.
    write_manifest("branch", "other-feature");

    // `--locked` must reject the mismatch and not touch the lock.
    let lock_before = fs::read_to_string(workspace_path.join("pixi.lock")).unwrap();
    let res = pixi.install().with_locked().await;
    assert!(
        res.is_err(),
        "`pixi install --locked` must fail when manifest's git ref drifts from the lock"
    );
    let lock_after = fs::read_to_string(workspace_path.join("pixi.lock")).unwrap();
    assert_eq!(
        lock_before, lock_after,
        "failed --locked install must leave the lock byte-identical"
    );
    assert_eq!(
        extract_git_build_sources(&pixi.lock_file().await.unwrap()),
        initial,
    );

    // A regular `pixi lock` updates the pin to the new ref.
    pixi.lock().await.unwrap();
    let new_sources = extract_git_build_sources(&pixi.lock_file().await.unwrap());
    assert_ne!(
        new_sources, initial,
        "`pixi lock` after a manifest git-ref change must produce a different pin"
    );

    // The follow-up `--locked` install accepts the refreshed lock.
    pixi.install().with_locked().await.unwrap();
    assert_eq!(
        extract_git_build_sources(&pixi.lock_file().await.unwrap()),
        new_sources,
    );
}

fn count_build_events(events: &[BackendEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, BackendEvent::CondaBuildV1Called))
        .count()
}

/// Regression test for PIX-1692: a relative `exclude-newer` cutoff must not
/// invalidate the source-build cache on consecutive `pixi install`s when the
/// source package itself has not changed.
#[tokio::test]
async fn install_with_relative_exclude_newer_does_not_rebuild_unchanged_source_packages() {
    setup_tracing();

    let (instantiator, mut observer) =
        ObservableBackend::instantiator(PassthroughBackend::instantiator());
    let backend_override = BackendOverride::from_memory(instantiator);
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    let source_dir = pixi.workspace_path().join("my-package");
    fs::create_dir_all(&source_dir).unwrap();

    fs::write(
        source_dir.join("pixi.toml"),
        r#"
[package]
name = "my-package"
version = "0.0.0"

[package.build]
backend = { name = "in-memory", version = "0.1.0" }
"#,
    )
    .unwrap();

    fs::write(
        pixi.manifest_path(),
        format!(
            r#"
[workspace]
name = "my-package"
channels = []
exclude-newer = "7d"
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
my-package = {{ path = "./my-package" }}
"#,
            Platform::current()
        ),
    )
    .unwrap();

    pixi.install().await.unwrap();
    let first_build_events = observer.events();
    assert_eq!(
        count_build_events(&first_build_events),
        1,
        "first install should build the source package once"
    );

    tokio::time::sleep(Duration::from_millis(10)).await;

    pixi.install().await.unwrap();
    let second_build_events = observer.events();
    assert_eq!(
        count_build_events(&second_build_events),
        0,
        "second install should reuse the existing build cache for an unchanged source package"
    );
}

fn simple_package_manifest(platform: Platform) -> String {
    format!(
        r#"
[workspace]
channels = []
platforms = ["{platform}"]
preview = ["pixi-build"]

[package]
name = "my-package"
version = "1.0.0"

[package.build]
backend = {{ name = "in-memory", version = "0.1.0" }}
"#
    )
}

/// `pixi publish` without a `to` argument must build the package and return
/// successfully without uploading anything.
#[tokio::test]
async fn test_publish_without_target_builds_but_does_not_upload() {
    setup_tracing();

    let (instantiator, mut observer) =
        ObservableBackend::instantiator(PassthroughBackend::instantiator());
    let pixi = PixiControl::from_manifest(&simple_package_manifest(Platform::current())).unwrap();

    publish::execute(publish::Args {
        backend_override: Some(BackendOverride::from_memory(instantiator)),
        config_cli: Default::default(),
        target_platform: Platform::current(),
        build_platform: Platform::current(),
        build_string_prefix: None,
        build_number: None,
        build_dir: None,
        clean: false,
        path: Some(pixi.manifest_path()),
        target_channel: None,
        target_dir: None,
        force: false,
        skip_existing: true,
        generate_attestation: false,
        variant: Vec::new(),
        variant_config: Vec::new(),
    })
    .await
    .expect("publish without target should succeed");

    assert!(
        !observer.build_events().is_empty(),
        "publish without target should still build the package"
    );
}

/// Regression test for #4761: `.pixi/.gitignore` must be created during
/// `sanity_check_workspace`, even when the publish itself fails because the
/// configured backend cannot be resolved. Without the gitignore, rattler-build
/// recurses into the workspace when source files reference the project root.
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_publish_creates_gitignore() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    // Manifest references a backend that cannot be resolved, so publish will
    // fail – but only after sanity_check_workspace has run.
    let manifest_content = format!(
        r#"
[workspace]
channels = []
platforms = ["{}"]
preview = ["pixi-build"]

[package]
name = "test-gitignore-publish"
version = "0.1.0"
description = "Test package for .gitignore creation during publish"

[package.build]
backend.name = "nonexistent-backend"
backend.version = "0.1.0"
"#,
        Platform::current(),
    );
    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    let gitignore_path = pixi.workspace().unwrap().pixi_dir().join(".gitignore");
    assert!(
        !gitignore_path.exists(),
        ".pixi/.gitignore should not exist before publish"
    );

    let _ = publish::execute(publish::Args {
        backend_override: None,
        config_cli: Default::default(),
        target_platform: Platform::current(),
        build_platform: Platform::current(),
        build_string_prefix: None,
        build_number: None,
        build_dir: None,
        clean: false,
        path: Some(pixi.manifest_path()),
        target_channel: None,
        target_dir: None,
        force: false,
        skip_existing: true,
        generate_attestation: false,
        variant: Vec::new(),
        variant_config: Vec::new(),
    })
    .await;

    assert!(
        gitignore_path.exists(),
        ".pixi/.gitignore was not created after publish"
    );
}
