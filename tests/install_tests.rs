mod common;

use std::{
    fs::{create_dir_all, File},
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

use common::{LockFileExt, PixiControl};
use pixi::{
    cli::{run, run::Args, LockFileUsageArgs},
    config::{Config, DetachedEnvironments},
    consts,
    consts::{DEFAULT_ENVIRONMENT_NAME, PIXI_UV_INSTALLER},
};
use pixi_manifest::FeatureName;
use rattler_conda_types::Platform;
use serial_test::serial;
use tempfile::TempDir;
use uv_toolchain::PythonEnvironment;

use crate::common::{
    builders::{string_from_iter, HasDependencyConfig},
    package_database::{Package, PackageDatabase},
};

/// Should add a python version to the environment and lock file that matches
/// the specified version and run it
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
            task: Some(string_from_iter(["python", "--version"])),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.trim(), "Python 3.11.0");
    assert!(result.stderr.is_empty());

    // Test for existence of environment file
    assert!(pixi
        .default_env_path()
        .unwrap()
        .join("conda-meta")
        .join(consts::ENVIRONMENT_FILE_NAME)
        .exists())
}

/// This is a test to check that creating incremental lock files works.
///
/// It works by using a fake channel that contains two packages: `foo` and
/// `bar`. `foo` depends on `bar` so adding a dependency on `foo` pulls in
/// `bar`. Initially only version `1` of both packages is added and a project is
/// created that depends on `foo >=1`. This select `foo@1` and `bar@1`.
/// Next, version 2 for both packages is added and the requirement in the
/// project is updated to `foo >=2`, this should then select `foo@1` but `bar`
/// should remain on version `1` even though version `2` is available. This is
/// because `bar` was previously locked to version `1` and it is still a valid
/// solution to keep using version `1` of bar.
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

    // Force using version 2 of `foo`. This should force `foo` to version `2` but
    // `bar` should still remaining on `1` because it was previously locked
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
async fn install_locked_with_config() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Overwrite install location to a target directory
    let mut config = Config::default();
    let target_dir = pixi.project_path().join("target");
    config.detached_environments = Some(DetachedEnvironments::Path(target_dir.clone()));
    create_dir_all(target_dir.clone()).unwrap();

    let config_path = pixi.project().unwrap().pixi_dir().join("config.toml");
    create_dir_all(config_path.parent().unwrap()).unwrap();

    let mut file = File::create(config_path).unwrap();
    file.write_all(toml_edit::ser::to_string(&config).unwrap().as_bytes())
        .unwrap();

    // Add and update lockfile with this version of python
    let python_version = if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "python==3.10.0"
    } else if cfg!(target_os = "windows") {
        // Abusing this test to also test the `add` function of older version of python
        // Before this wasn't possible because uv queried the python interpreter, even
        // without pypi dependencies.
        "python==3.6.0"
    } else {
        "python==2.7.15"
    };

    pixi.add(python_version).await.unwrap();

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
        python_version
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

    // Task command depends on the OS
    let which_command = if cfg!(target_os = "windows") {
        "where python"
    } else {
        "which python"
    };

    // Verify that the folders are present in the target directory using a task.
    pixi.tasks()
        .add("which_python".into(), None, FeatureName::Default)
        .with_commands([which_command])
        .execute()
        .unwrap();

    let result = pixi
        .run(Args {
            task: Some(vec!["which_python".to_string()]),
            manifest_path: None,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);

    // Check for correct path in most important path
    let line = result.stdout.lines().next().unwrap();
    let target_dir_canonical = target_dir.canonicalize().unwrap();
    let line_path = PathBuf::from(line).canonicalize().unwrap();
    assert!(line_path.starts_with(target_dir_canonical));
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
            task: Some(string_from_iter(["python", "--version"])),
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

    // Current interpreter and venv
    let interpreter = uv_toolchain::Interpreter::query(python, cache).unwrap();
    uv_toolchain::PythonEnvironment::from_interpreter(interpreter)
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
    assert!(pixi.lock_file().await.unwrap().contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.11"
    ));

    let prefix = pixi.default_env_path().unwrap();

    let cache = uv_cache::Cache::temp().unwrap();

    // Check if site-packages has entries
    let env = create_uv_environment(&prefix, &cache);
    let installed_311 = uv_installer::SitePackages::from_environment(&env).unwrap();
    assert!(installed_311.iter().count() > 0);

    // sleep for a few seconds to make sure we can remove stuff (Windows file system
    // issues)
    #[cfg(target_os = "windows")]
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Reinstall python
    pixi.add("python==3.12").with_install(true).await.unwrap();
    assert!(pixi.lock_file().await.unwrap().contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.12"
    ));

    // Check if site-packages has entries, should be empty now
    let installed_312 = uv_installer::SitePackages::from_environment(&env).unwrap();

    if cfg!(not(target_os = "windows")) {
        // On non-windows the site-packages should be empty
        assert_eq!(installed_312.iter().count(), 0);
    } else {
        // Windows should still contain some packages
        // This is because the site-packages is not prefixed with the python version
        assert!(installed_312.iter().count() > 0);
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
    pixi.add("flask[dotenv]")
        .with_install(true)
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    let prefix = pixi.default_env_path().unwrap();

    let cache = uv_cache::Cache::temp().unwrap();

    // Check if site-packages has entries
    let env = create_uv_environment(&prefix, &cache);
    let installed_311 = uv_installer::SitePackages::from_environment(&env).unwrap();
    assert!(installed_311.iter().count() > 0);

    pixi.remove("flask[dotenv]")
        .set_type(pixi::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    let installed_311 = uv_installer::SitePackages::from_environment(&env).unwrap();
    assert!(installed_311.iter().count() == 0);
}

#[tokio::test]
async fn test_channels_changed() {
    // Write a channel with a package `bar` with only one version
    let mut package_database_a = PackageDatabase::default();
    package_database_a.add_package(Package::build("bar", "2").finish());
    let channel_a = package_database_a.into_channel().await.unwrap();

    // Write another channel with a package `bar` with only one version but another
    // one.
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

    // Get an up-to-date lockfile and verify that bar version 2 was selected from
    // channel `a`.
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

    // Get an up-to-date lockfile and verify that bar version 1 was now selected
    // from channel `b`.
    let lock_file = pixi.up_to_date_lock_file().await.unwrap();
    assert!(lock_file.contains_match_spec(DEFAULT_ENVIRONMENT_NAME, platform, "bar ==1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_conda_meta_history() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.install().await.unwrap();

    let prefix = pixi.default_env_path().unwrap();
    let conda_meta_history_file = prefix.join("conda-meta/history");

    assert!(conda_meta_history_file.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn minimal_lockfile_update_pypi() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();

    // Add pypi dependencies which are not the latest options
    pixi.add_multiple(vec!["uvicorn==0.28.0", "click==7.1.2"])
        .set_type(pixi::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Check the locked click dependencies
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pep508_requirement(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        pep508_rs::Requirement::from_str("click==7.1.2").unwrap()
    ));

    // Widening the click version to allow for the latest version
    pixi.add_multiple(vec!["uvicorn==0.29.0", "click"])
        .set_type(pixi::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // `click` should not be updated to a higher version.
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pep508_requirement(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        pep508_rs::Requirement::from_str("click==7.1.2").unwrap()
    ));
}

/// Create a test that installs a package with pixi
/// change the installer and see if it does not touch the package
/// then change the installer back and see if it reinstalls the package
/// with a new version
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_installer_name() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();
    pixi.add("click==8.0.0")
        .set_type(pixi::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Get the correct dist-info folder
    let dist_info = if cfg!(not(target_os = "windows")) {
        pixi.default_env_path()
            .unwrap()
            .join("lib/python3.11/site-packages/click-8.0.0.dist-info")
    } else {
        let default_env_path = pixi.default_env_path().unwrap();
        default_env_path.join("Lib/site-packages/click-8.0.0.dist-info")
    };
    // Check that installer name is uv-pixi
    assert!(dist_info.exists(), "{dist_info:?} does not exist");
    let installer = dist_info.join("INSTALLER");
    let installer = std::fs::read_to_string(installer).unwrap();
    assert_eq!(installer, PIXI_UV_INSTALLER);

    // Write a new installer name to the INSTALLER file
    // so that we fake that it is not installed by pixi
    std::fs::write(dist_info.join("INSTALLER"), "not-pixi").unwrap();
    pixi.remove("click==8.0.0")
        .with_install(true)
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    // dist info folder should still exists
    // and should have the old installer name
    // we know that pixi did not touch the package
    assert!(dist_info.exists());
    let installer = dist_info.join("INSTALLER");
    let installer = std::fs::read_to_string(installer).unwrap();
    assert_eq!(installer, "not-pixi");

    // re-manage the package by adding it, this should cause a reinstall
    pixi.add("click==8.0.0")
        .set_type(pixi::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();
    let installer = dist_info.join("INSTALLER");
    let installer = std::fs::read_to_string(installer).unwrap();
    assert_eq!(installer, PIXI_UV_INSTALLER);
}
