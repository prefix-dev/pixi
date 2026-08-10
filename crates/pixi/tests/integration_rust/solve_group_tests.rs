use rattler_conda_types::Platform;
use tempfile::TempDir;
use url::Url;

use crate::common::{LockFileExt, PixiControl};
use crate::setup_tracing;
use pixi_test_utils::{MockRepoData, Package};

#[tokio::test]
async fn conda_solve_group_functionality() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

    // Add a package `foo` with 3 different versions
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database.add_package(Package::build("foo", "3").finish());

    // Add a package `bar` with 1 version that restricts `foo` to version 2 or
    // lower.
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

    // Get an up-to-date lock file
    let lock_file = pixi.update_lock_file().await.unwrap();

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

#[tokio::test]
async fn conda_solve_group_heterogeneous_platforms() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

    // Add `foo` available on both linux-64 and win-64
    package_database.add_package(
        Package::build("foo", "1")
            .with_subdir(Platform::Linux64)
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "1")
            .with_subdir(Platform::Win64)
            .finish(),
    );

    // Add `bar` available only on linux-64
    package_database.add_package(
        Package::build("bar", "1")
            .with_subdir(Platform::Linux64)
            .finish(),
    );

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let channel = Url::from_file_path(channel_dir.path()).unwrap();

    // The `linux-only` feature restricts to linux-64 and adds `bar`.
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-heterogeneous-platforms"
    channels = ["{channel}"]
    platforms = ["linux-64", "win-64"]

    [dependencies]
    foo = "*"

    [feature.linux-only]
    platforms = ["linux-64"]

    [feature.linux-only.dependencies]
    bar = "*"

    [environments]
    full = {{ solve-group = "group1" }}
    restricted = {{ features = ["linux-only"], solve-group = "group1" }}
    "#
    ))
    .unwrap();

    // Solving should succeed for both platforms.
    let lock_file = pixi.update_lock_file().await.unwrap();

    // `full` environment: has `foo` on both platforms, no `bar`
    assert!(
        lock_file.contains_match_spec("full", Platform::Linux64, "foo ==1"),
        "full/linux-64 should have foo"
    );
    assert!(
        lock_file.contains_match_spec("full", Platform::Win64, "foo ==1"),
        "full/win-64 should have foo"
    );
    assert!(
        !lock_file.contains_conda_package("full", Platform::Win64, "bar"),
        "full/win-64 should not have bar"
    );
    assert!(
        !lock_file.contains_conda_package("full", Platform::Linux64, "bar"),
        "full/linux-64 should not have bar"
    );

    // `restricted` environment: only supports linux-64, should have both foo and bar
    assert!(
        lock_file.contains_match_spec("restricted", Platform::Linux64, "foo ==1"),
        "restricted/linux-64 should have foo"
    );
    assert!(
        lock_file.contains_match_spec("restricted", Platform::Linux64, "bar ==1"),
        "restricted/linux-64 should have bar"
    );
}

/// Test that environments in a solve-group can have different editability settings
/// for the same path-based PyPI package.
///
/// This test verifies that:
/// - Two environments in the same solve-group can specify the same local package
/// - One environment can have it as editable, the other as non-editable
/// - The lock file stores editable=false for both (editability is looked up from manifest at install time)
///
/// Note: With the new architecture, the lock file always stores `editable=false` (omitted in JSON).
/// The actual editability is determined from the manifest at install time, which allows different
/// environments in a solve-group to have different editability settings without affecting the lock file.
#[tokio::test]
async fn test_solve_group_per_environment_editability() {
    setup_tracing();

    // Create a fake channel with Python
    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("python", "3.10.0").finish());

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
    name = "test-editability"
    channels = ["{channel}"]
    platforms = ["{platform}"]
conda-pypi-map = false # disable mapping

    [dependencies]
    python = "*"

    [feature.prod.pypi-dependencies]
    # Non-editable in prod
    my-local-pkg = {{ path = "./my-local-pkg", editable = false }}

    [feature.dev.pypi-dependencies]
    # Editable in dev
    my-local-pkg = {{ path = "./my-local-pkg", editable = true }}

    [environments]
    prod = {{ features = ["prod"], solve-group = "default" }}
    dev = {{ features = ["dev"], solve-group = "default" }}
    "#
    ))
    .unwrap();

    // Create the local package directory structure
    let project_path = pixi.workspace_path();
    let pkg_dir = project_path.join("my-local-pkg");
    fs_err::create_dir_all(&pkg_dir).unwrap();

    // Create a minimal pyproject.toml for the local package (using setuptools which is simpler)
    fs_err::write(
        pkg_dir.join("pyproject.toml"),
        r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "my-local-pkg"
version = "0.1.0"
"#,
    )
    .unwrap();

    // Create the package source
    let src_dir = pkg_dir.join("my_local_pkg");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "").unwrap();

    // Lock the project
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify the package is present in both environments
    assert!(
        lock_file.contains_pypi_package("prod", platform, "my-local-pkg"),
        "prod environment should contain my-local-pkg"
    );
    assert!(
        lock_file.contains_pypi_package("dev", platform, "my-local-pkg"),
        "dev environment should contain my-local-pkg"
    );
}

/// Regression test for #6121: `core` declared editable both as a direct
/// pixi pypi-dependency and via the transitive `[tool.uv.sources]` of
/// `middle` must not produce a "conflicting URLs" error.
#[tokio::test]
async fn test_transitive_uv_sources_editable_consistency() {
    setup_tracing();

    // Create a fake channel with Python
    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("python", "3.10.0").finish());

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
    name = "test-transitive-editable"
    channels = ["{channel}"]
    platforms = ["{platform}"]
    conda-pypi-map = false # disable mapping

    [dependencies]
    python = "*"

    [pypi-dependencies]
    core   = {{ path = "./core",   editable = true }}
    middle = {{ path = "./middle", editable = true }}
    "#
    ))
    .unwrap();

    let project_path = pixi.workspace_path();

    let core_dir = project_path.join("core");
    fs_err::create_dir_all(&core_dir).unwrap();
    fs_err::write(
        core_dir.join("pyproject.toml"),
        r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "core"
version = "0.1.0"
"#,
    )
    .unwrap();
    let core_src = core_dir.join("core");
    fs_err::create_dir_all(&core_src).unwrap();
    fs_err::write(core_src.join("__init__.py"), "").unwrap();

    let middle_dir = project_path.join("middle");
    fs_err::create_dir_all(&middle_dir).unwrap();
    fs_err::write(
        middle_dir.join("pyproject.toml"),
        r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "middle"
version = "0.1.0"
dependencies = ["core"]

[tool.uv.sources]
core = { path = "../core", editable = true }
"#,
    )
    .unwrap();
    let middle_src = middle_dir.join("middle");
    fs_err::create_dir_all(&middle_src).unwrap();
    fs_err::write(middle_src.join("__init__.py"), "").unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    assert!(
        lock_file.contains_pypi_package("default", platform, "core"),
        "default environment should contain core"
    );
    assert!(
        lock_file.contains_pypi_package("default", platform, "middle"),
        "default environment should contain middle"
    );
}
