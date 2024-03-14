mod common;

use std::path::Path;

use crate::common::builders::string_from_iter;
use crate::common::package_database::{Package, PackageDatabase};
use common::{LockFileExt, PixiControl};
use pixi::cli::{run, LockFileUsageArgs};
use pixi::consts::DEFAULT_ENVIRONMENT_NAME;
use rattler_conda_types::Platform;
use serial_test::serial;
use tempfile::TempDir;
use uv_interpreter::PythonEnvironment;

/// Should add a python version to the environment and lock file that matches the specified version
/// and run it
#[tokio::test]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_run_python() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.add("python==3.11.0").with_install(true).await.unwrap();

    // Check if lock has python version
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.11.0"
    ));

    // Check if python is installed and can be run
    let result = pixi
        .run(run::Args {
            task: string_from_iter(["python", "--version"]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.trim(), "Python 3.11.0");
    assert!(result.stderr.is_empty());
}

/// This is a test to check that creating incremental lock files works.
///
/// It works by using a fake channel that contains two packages: `foo` and `bar`. `foo` depends on
/// `bar` so adding a dependency on `foo` pulls in `bar`. Initially only version `1` of both
/// packages is added and a project is created that depends on `foo >=1`. This select `foo@1` and
/// `bar@1`.
/// Next, version 2 for both packages is added and the requirement in the project is updated to
/// `foo >=2`, this should then select `foo@1` but `bar` should remain on version `1` even though
/// version `2` is available. This is because `bar` was previously locked to version `1` and it is
/// still a valid solution to keep using version `1` of bar.
#[tokio::test]
async fn test_incremental_lock_file() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(
        Package::build("foo", "1")
            .with_dependency("bar >=1")
            .finish(),
    );

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a dependency on `foo`
    pixi.add("foo").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "foo ==1"));
    assert!(lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "bar ==1"));

    // Add version 2 of both `foo` and `bar`.
    package_database.add_package(Package::build("bar", "2").finish());
    package_database.add_package(
        Package::build("foo", "2")
            .with_dependency("bar >=1")
            .finish(),
    );
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Force using version 2 of `foo`. This should force `foo` to version `2` but `bar` should still
    // remaining on `1` because it was previously locked
    pixi.add("foo >=2").await.unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(
        lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "foo ==2"),
        "expected `foo` to be on version 2 because we changed the requirement"
    );
    assert!(
        lock.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, Platform::current(), "bar ==1"),
        "expected `bar` to remain locked to version 1."
    );
}

/// Test the `pixi install --locked` functionality.
#[tokio::test]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_locked() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.10.0").await.unwrap();

    // Add new version of python only to the manifest
    pixi.add("python==3.9.0")
        .without_lockfile_update()
        .await
        .unwrap();

    assert!(pixi.install().with_locked().await.is_err(), "should error when installing with locked but there is a mismatch in the dependencies and the lockfile.");

    // Check if it didn't accidentally update the lockfile
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.10.0"
    ));

    // After an install with lockfile update the locked install should succeed.
    pixi.install().await.unwrap();
    pixi.install().with_locked().await.unwrap();

    // Check if lock has python version updated
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.9.0"
    ));
}

/// Test `pixi install/run --frozen` functionality
#[tokio::test]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_frozen() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.9.1").await.unwrap();

    // Add new version of python only to the manifest
    pixi.add("python==3.10.1")
        .without_lockfile_update()
        .await
        .unwrap();

    pixi.install().with_frozen().await.unwrap();

    // Check if it didn't accidentally update the lockfile
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.9.1"
    ));

    // Check if running with frozen doesn't suddenly install the latest update.
    let result = pixi
        .run(run::Args {
            lock_file_usage: LockFileUsageArgs {
                frozen: true,
                ..Default::default()
            },
            task: string_from_iter(["python", "--version"]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.trim(), "Python 3.9.1");
    assert!(result.stderr.is_empty());
}

fn create_uv_environment(prefix: &Path, cache: &uv_cache::Cache) -> PythonEnvironment {
    let python = if cfg!(target_os = "windows") {
        prefix.join("python.exe")
    } else {
        prefix.join("bin/python")
    };
    let platform = platform_host::Platform::current().unwrap();
    // Current interpreter and venv
    let interpreter = uv_interpreter::Interpreter::query(&python, platform, cache).unwrap();
    uv_interpreter::PythonEnvironment::from_interpreter(interpreter)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn pypi_reinstall_python() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();

    // Add flask from pypi
    pixi.add("flask")
        .with_install(true)
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    let prefix = pixi.project().unwrap().root().join(".pixi/envs/default");

    let cache = uv_cache::Cache::temp().unwrap();

    // Check if site-packages has entries
    let env = create_uv_environment(&prefix, &cache);
    let installed_311 = uv_installer::SitePackages::from_executable(&env).unwrap();
    assert!(installed_311.iter().count() > 0);

    // Reinstall python
    pixi.add("python==3.12").with_install(true).await.unwrap();

    // Check if site-packages has entries, should be empty now
    let installed_311 = uv_installer::SitePackages::from_executable(&env).unwrap();

    if cfg!(not(target_os = "windows")) {
        // On non-windows the site-packages should be empty
        assert!(installed_311.iter().count() == 0);
    } else {
        // Windows should still contain some packages
        // This is because the site-packages is not prefixed with the python version
        assert!(installed_311.iter().count() > 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
// Check if we add and remove a pypi package that the site-packages is cleared
async fn pypi_add_remove() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();

    // Add flask from pypi
    pixi.add("flask")
        .with_install(true)
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    let prefix = pixi.project().unwrap().root().join(".pixi/envs/default");

    let cache = uv_cache::Cache::temp().unwrap();

    // Check if site-packages has entries
    let env = create_uv_environment(&prefix, &cache);
    let installed_311 = uv_installer::SitePackages::from_executable(&env).unwrap();
    assert!(installed_311.iter().count() > 0);

    pixi.remove("flask")
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    let installed_311 = uv_installer::SitePackages::from_executable(&env).unwrap();
    assert!(installed_311.iter().count() == 0);
}

#[tokio::test]
async fn test_channels_changed() {
    // Write a channel with a package `bar` with only one version
    let mut package_database_a = PackageDatabase::default();
    package_database_a.add_package(Package::build("bar", "2").finish());
    let channel_a = package_database_a.into_channel().await.unwrap();

    // Write another channel with a package `bar` with only one version but another one.
    let mut package_database_b = PackageDatabase::default();
    package_database_b.add_package(Package::build("bar", "1").finish());
    let channel_b = package_database_b.into_channel().await.unwrap();

    let platform = Platform::current();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-channel-change"
    channels = ["{channel_a}"]
    platforms = ["{platform}"]

    [dependencies]
    bar = "*"
    "#,
        channel_a = channel_a.url(),
    ))
    .unwrap();

    // Get an up-to-date lockfile and verify that bar version 2 was selected from channel `a`.
    let lock_file = pixi.up_to_date_lock_file().await.unwrap();
    assert!(lock_file.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, platform, "bar ==2"));

    // Switch the channel around
    let platform = Platform::current();
    pixi.update_manifest(&format!(
        r#"
    [project]
    name = "test-channel-change"
    channels = ["{channel_b}"]
    platforms = ["{platform}"]

    [dependencies]
    bar = "*"
    "#,
        channel_b = channel_b.url()
    ))
    .unwrap();

    // Get an up-to-date lockfile and verify that bar version 1 was now selected from channel `b`.
    let lock_file = pixi.up_to_date_lock_file().await.unwrap();
    assert!(lock_file.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, platform, "bar ==1"));
}
