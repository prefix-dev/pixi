use fs_err as fs;
use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::BackendOverride;
use pixi_consts::consts;
use rattler_conda_types::Platform;

use crate::{
    common::{
        LockFileExt, PixiControl,
        package_database::{Package, PackageDatabase},
    },
    setup_tracing,
};

/// Helper function to create a package database with common test dependencies
fn create_test_package_database() -> PackageDatabase {
    let mut db = PackageDatabase::default();

    // Add common dependencies that our test packages will need
    db.add_package(Package::build("cmake", "3.20.0").finish());
    db.add_package(Package::build("make", "4.3.0").finish());
    db.add_package(Package::build("gcc", "11.0.0").finish());
    db.add_package(Package::build("openssl", "3.0.0").finish());
    db.add_package(Package::build("zlib", "1.2.11").finish());
    db.add_package(Package::build("python", "3.9.0").finish());
    db.add_package(Package::build("python", "3.10.0").finish());
    db.add_package(Package::build("python", "3.11.0").finish());
    db.add_package(Package::build("python", "3.12.0").finish());
    db.add_package(Package::build("python", "3.13.0").finish());
    db.add_package(Package::build("numpy", "1.21.0").finish());
    db.add_package(Package::build("requests", "2.26.0").finish());

    db
}

/// Helper function to create a source package directory with a pixi.toml
fn create_source_package(
    base_dir: &std::path::Path,
    name: &str,
    version: &str,
    dependencies: &str,
) -> std::path::PathBuf {
    let package_dir = base_dir.join(name);
    fs::create_dir_all(&package_dir).unwrap();

    let pixi_toml_content = format!(
        r#"
[package]
name = "{}"
version = "{}"

[package.build]
backend = {{ name = "in-memory", version = "0.1.0" }}

{}"#,
        name, version, dependencies
    );

    fs::write(package_dir.join("pixi.toml"), pixi_toml_content).unwrap();
    package_dir
}

/// Test that develop dependencies are correctly expanded and included in the lock-file
#[tokio::test]
async fn test_develop_dependencies_basic() {
    setup_tracing();

    // Create a package database with common dependencies
    let package_database = create_test_package_database();

    // Convert to channel
    let channel = package_database.into_channel().await.unwrap();

    // Create a PixiControl instance with PassthroughBackend
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a source package with dependencies
    let _my_package = create_source_package(
        pixi.workspace_path(),
        "my-package",
        "1.0.0",
        r#"
[package.build-dependencies]
cmake = ">=3.0"

[package.host-dependencies]
openssl = ">=2.0"

[package.run-dependencies]
python = ">=3.8"
"#,
    );

    // Create a manifest with develop dependencies
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[develop]
my-package = {{ path = "./my-package" }}
"#,
        channel.url(),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that the dependencies of my-package are in the lock-file
    // but my-package itself is NOT built/installed
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "cmake",
        ),
        "cmake should be in the lock-file (build dependency of develop package)"
    );

    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "openssl",
        ),
        "openssl should be in the lock-file (host dependency of develop package)"
    );

    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "python",
        ),
        "python should be in the lock-file (run dependency of develop package)"
    );

    assert!(
        !lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "my-package",
        ),
        "my-package itself should NOT be in the lock-file (it's a develop dependency)"
    );
}

/// Test that source dependencies of develop packages are correctly expanded
#[tokio::test]
async fn test_develop_dependencies_with_source_dependencies() {
    setup_tracing();

    // Create a package database
    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create package-b inside the workspace
    let package_b_path = create_source_package(
        pixi.workspace_path(),
        "package-b",
        "1.0.0",
        r#"
[package.run-dependencies]
numpy = ">=1.0"
"#,
    );

    // Create package-a inside the workspace that depends on package-b via path
    let _package_a = create_source_package(
        pixi.workspace_path(),
        "package-a",
        "1.0.0",
        &format!(
            r#"
[package.build-dependencies]
gcc = ">=9.0"

[package.run-dependencies]
package-b = {{ path = "{}" }}
requests = ">=2.0"
"#,
            package_b_path.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    // Create a manifest with package-a as a develop dependency
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[develop]
package-a = {{ path = "./package-a" }}
"#,
        channel.url(),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file - this should correctly resolve the relative path from package-a to package-b
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that package-a's dependencies are resolved correctly
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "gcc",
        ),
        "gcc should be in the lock-file (build dependency of package-a)"
    );

    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "requests",
        ),
        "requests should be in the lock-file (run dependency of package-a)"
    );

    // Verify that package-b's dependencies are also resolved
    // This tests that the relative path ../package-b was correctly resolved
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "numpy",
        ),
        "numpy should be in the lock-file (run dependency of package-b, which is a source dependency of package-a)"
    );

    // Verify that package-a is NOT built (it's a develop dependency)
    assert!(
        !lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "package-a",
        ),
        "package-a should NOT be in the lock-file (it's a develop dependency)"
    );

    // Note: package-b WILL be in the lock-file because it's a source dependency
    // of package-a. Source dependencies need to be built to extract their dependencies.
    // This is expected behavior - only the direct develop dependencies are not built.
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "package-b",
        ),
        "package-b SHOULD be in the lock-file (it's a source dependency that needs to be built)"
    );
}

/// Test that when multiple develop dependencies reference each other, they are correctly filtered
#[tokio::test]
async fn test_develop_dependencies_with_cross_references() {
    setup_tracing();

    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create package-y in the workspace
    let package_y_path = create_source_package(
        pixi.workspace_path(),
        "package-y",
        "1.0.0",
        r#"
[package.host-dependencies]
openssl = ">=2.0"
"#,
    );

    // Create package-x that depends on package-y
    let _package_x = create_source_package(
        pixi.workspace_path(),
        "package-x",
        "1.0.0",
        &format!(
            r#"
[package.build-dependencies]
cmake = ">=3.0"

[package.run-dependencies]
package-y = {{ path = "{}" }}
"#,
            package_y_path.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    // Add BOTH as develop dependencies
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[develop]
package-x = {{ path = "./package-x" }}
package-y = {{ path = "{}" }}
"#,
        channel.url(),
        Platform::current(),
        package_y_path.to_string_lossy().replace('\\', "\\\\")
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that the dependencies are present
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "cmake",
        ),
        "cmake should be in the lock-file (build dependency of package-x)"
    );

    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "openssl",
        ),
        "openssl should be in the lock-file (host dependency of package-y)"
    );

    // Verify that neither package-x nor package-y are in the lock-file
    // This is the key test: package-y is referenced by package-x, but since both are
    // develop dependencies, package-y should be filtered out from package-x's dependencies
    assert!(
        !lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "package-x",
        ),
        "package-x should NOT be in the lock-file (it's a develop dependency)"
    );

    assert!(
        !lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "package-y",
        ),
        "package-y should NOT be in the lock-file (it's a develop dependency)"
    );
}

/// Test that feature-specific develop dependencies work correctly
#[tokio::test]
async fn test_develop_dependencies_in_features() {
    setup_tracing();

    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a package for the feature
    let _feature_package = create_source_package(
        pixi.workspace_path(),
        "feature-package",
        "1.0.0",
        r#"
[package.run-dependencies]
zlib = ">=1.0"
"#,
    );

    // Create a manifest with feature-specific develop dependencies
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[environments]
test = ["test-feature"]

[feature.test-feature.develop]
feature-package = {{ path = "./feature-package" }}
"#,
        channel.url(),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that zlib is in the "test" environment but not in the default environment
    assert!(
        lock_file.contains_conda_package("test", Platform::current(), "zlib",),
        "zlib should be in the test environment lock-file (run dependency of feature-package)"
    );

    assert!(
        !lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "zlib",
        ),
        "zlib should NOT be in the default environment (feature-package is only in test-feature)"
    );

    // Verify that feature-package itself is not built
    assert!(
        !lock_file.contains_conda_package("test", Platform::current(), "feature-package",),
        "feature-package should NOT be in the lock-file (it's a develop dependency)"
    );
}

/// Test that a source package can be listed both in [develop] and in dependencies
/// without causing conflicts (the package is essentially included twice, once as a develop dep
/// and once as a regular source dep)
#[tokio::test]
async fn test_develop_and_regular_dependency_same_package() {
    setup_tracing();

    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a shared package that will be both a develop dependency and a regular dependency
    let shared_package_path = create_source_package(
        pixi.workspace_path(),
        "shared-package",
        "1.0.0",
        r#"
[package.host-dependencies]
python = ">=3.8"
"#,
    );

    // Create another package that depends on shared-package as a regular source dependency
    let _dependent_package = create_source_package(
        pixi.workspace_path(),
        "dependent-package",
        "1.0.0",
        &format!(
            r#"
[package.run-dependencies]
shared-package = {{ path = "{}" }}
numpy = ">=1.0"
"#,
            shared_package_path.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    // Create a manifest that:
    // 1. Lists shared-package as a develop dependency
    // 2. Lists dependent-package as a regular source dependency
    // This means shared-package appears both as a develop dep and as a transitive source dep
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
dependent-package = {{ path = "./dependent-package" }}

[develop]
shared-package = {{ path = "{}" }}
"#,
        channel.url(),
        Platform::current(),
        shared_package_path.to_string_lossy().replace('\\', "\\\\")
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file - this should work without conflicts
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that python is in the lock-file (from shared-package's dependencies)
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "python",
        ),
        "python should be in the lock-file (run dependency of shared-package)"
    );

    // Verify that numpy is in the lock-file (from dependent-package's dependencies)
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "numpy",
        ),
        "numpy should be in the lock-file (run dependency of dependent-package)"
    );

    // Verify that dependent-package IS in the lock-file (it's a regular source dependency)
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "dependent-package",
        ),
        "dependent-package SHOULD be in the lock-file (it's a regular source dependency)"
    );

    // Key assertion: shared-package WILL appear in the lock-file as a built package
    // because it's a source dependency of dependent-package.
    // The fact that it's also in [develop] doesn't prevent it from being built when
    // it's needed as a dependency of another package.
    // This is correct behavior - [develop] means "install my dependencies without building me",
    // but if another package needs it built, it will be built.
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "shared-package",
        ),
        "shared-package SHOULD be in the lock-file (it's built as a source dependency of dependent-package)"
    );
}

/// Test that platform-specific develop dependencies work correctly
#[tokio::test]
async fn test_develop_dependencies_platform_specific() {
    setup_tracing();

    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create a package for the current platform
    let _platform_package = create_source_package(
        pixi.workspace_path(),
        "platform-package",
        "1.0.0",
        r#"
[package.run-dependencies]
make = ">=4.0"
"#,
    );

    // Create a manifest with platform-specific develop dependencies
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[target.{}.develop]
platform-package = {{ path = "./platform-package" }}
"#,
        channel.url(),
        Platform::current(),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that make is in the lock-file for the current platform
    assert!(
        lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "make",
        ),
        "make should be in the lock-file (run dependency of platform-package)"
    );

    // Verify that platform-package itself is not built
    assert!(
        !lock_file.contains_conda_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "platform-package",
        ),
        "platform-package should NOT be in the lock-file (it's a develop dependency)"
    );
}

/// Test that variant selection chooses the highest matching version
/// When python = "*" with variants [3.10, 3.12], should select 3.12 even though 3.13 exists
#[tokio::test]
async fn test_develop_dependency_variant_selection() {
    setup_tracing();
    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    // Create the test directory
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create the variant-python-package directory
    create_source_package(
        pixi.workspace_path(),
        "variant-python-package",
        "0.1.0",
        r#"
[package.run-dependencies]
python = "*"
        "#,
    );

    // Create a manifest with develop dependencies and variants
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]

[develop]
variant-python-package = {{ path = "./variant-python-package" }}

[workspace.build-variants]
python = ["3.10", "3.12"]
"#,
        channel.url(),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that python 3.12 is in the lock-file (highest variant)
    assert!(
        lock_file.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "python ==3.12.0",
        ),
        "Should select python 3.12 (highest available variant), not 3.13"
    );
}

/// Test that variant selection is constrained by regular dependencies
/// When python = "*" with variants [3.10, 3.12], but dependencies require <3.12, should select 3.10
#[tokio::test]
async fn test_develop_dependency_variant_constrained_by_dependencies() {
    setup_tracing();
    let package_database = create_test_package_database();

    let channel = package_database.into_channel().await.unwrap();

    // Create the test directory
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());
    let pixi = PixiControl::new()
        .unwrap()
        .with_backend_override(backend_override);

    // Create the variant-python-package directory
    create_source_package(
        pixi.workspace_path(),
        "variant-python-package",
        "0.1.0",
        r#"
[package.run-dependencies]
python = "*"
        "#,
    );

    // Create a manifest with develop dependencies, variants, and a constraining dependency
    let manifest_content = format!(
        r#"
[workspace]
channels = ["{}"]
platforms = ["{}"]
preview = ["pixi-build"]

[dependencies]
python = "<3.12"

[develop]
variant-python-package = {{ path = "./variant-python-package" }}

[workspace.build-variants]
python = ["3.10", "3.12"]
"#,
        channel.url(),
        Platform::current()
    );

    fs::write(pixi.manifest_path(), manifest_content).unwrap();

    // Update the lock-file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify that python 3.10 is in the lock-file (constrained by dependency)
    assert!(
        lock_file.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "python ==3.10.0",
        ),
        "Should select python 3.10 (constrained by dependency <3.12), not 3.12"
    );
}
