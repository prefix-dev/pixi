use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::LazyLock,
};

use fs_err::tokio as tokio_fs;
use pixi_cli::run::{self, Args};
use pixi_cli::{
    LockFileUsageConfig,
    cli_config::{LockAndInstallConfig, LockFileUpdateConfig, WorkspaceConfig},
};
use pixi_config::{Config, DetachedEnvironments};
use pixi_consts::consts;
use pixi_core::InstallFilter;
use pixi_core::{
    UpdateLockFileOptions, Workspace,
    environment::LockFileUsage,
    lock_file::{CondaPrefixUpdater, ReinstallPackages, UpdateMode},
    workspace::{HasWorkspaceRef, grouped_environment::GroupedEnvironment},
};
use pixi_manifest::{FeatureName, FeaturesExt};
use pixi_record::PixiRecord;
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use tempfile::{TempDir, tempdir};
use tokio::{fs, task::JoinSet};
use url::Url;
use uv_configuration::RAYON_INITIALIZE;
use uv_pep508::PackageName;
use uv_python::PythonEnvironment;

use crate::common::{
    LockFileExt, PixiControl,
    builders::{
        HasDependencyConfig, HasLockFileUpdateConfig, HasNoInstallConfig, string_from_iter,
    },
    package_database::{Package, PackageDatabase},
};
use crate::setup_tracing;

/// Should add a python version to the environment and lock file that matches
/// the specified version and run it
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_run_python() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.add("python==3.11.0").with_install(true).await.unwrap();

    // Check if lock has python version
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
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

    // Test for existence of environment file
    assert!(
        pixi.default_env_path()
            .unwrap()
            .join("conda-meta")
            .join(consts::ENVIRONMENT_FILE_NAME)
            .exists()
    )
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
    setup_tracing();

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
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "bar ==1"
    ));

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
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we changed the requirement"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "bar ==1"
        ),
        "expected `bar` to remain locked to version 1."
    );
}

/// Test the `pixi install --locked` functionality.
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_locked_with_config() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Overwrite install location to a target directory
    let mut config = Config::default();
    let target_dir = pixi.workspace_path().join("target");
    config.detached_environments = Some(DetachedEnvironments::Path(target_dir.clone()));
    fs_err::create_dir_all(target_dir.clone()).unwrap();

    let config_path = pixi.workspace().unwrap().pixi_dir().join("config.toml");
    fs_err::create_dir_all(config_path.parent().unwrap()).unwrap();

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
        .with_frozen(true)
        .with_install(false)
        .await
        .unwrap();

    assert!(
        pixi.install().with_locked().await.is_err(),
        "should error when installing with locked but there is a mismatch in the dependencies and the lockfile."
    );

    // Check if it didn't accidentally update the lockfile
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        python_version
    ));

    // After an install with lockfile update the locked install should succeed.
    pixi.install().await.unwrap();
    pixi.install().with_locked().await.unwrap();

    // Check if lock has python version updated
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
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
        .add("which_python".into(), None, FeatureName::default())
        .with_commands([which_command])
        .execute()
        .await
        .unwrap();

    let result = pixi
        .run(Args {
            task: vec!["which_python".to_string()],
            workspace_config: WorkspaceConfig {
                manifest_path: None,
            },
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
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_frozen() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.9.1").await.unwrap();

    // Add new version of python only to the manifest
    pixi.add("python==3.10.1")
        .with_frozen(true)
        .with_install(false)
        .await
        .unwrap();

    pixi.install().with_frozen().await.unwrap();

    // Check if it didn't accidentally update the lockfile
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "python==3.9.1"
    ));

    // Check if running with frozen doesn't suddenly install the latest update.
    let result = pixi
        .run(run::Args {
            lock_and_install_config: LockAndInstallConfig {
                lock_file_update_config: LockFileUpdateConfig {
                    lock_file_usage: LockFileUsageConfig {
                        frozen: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
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

    // Current interpreter and venv
    let interpreter = uv_python::Interpreter::query(python, cache).unwrap();
    uv_python::PythonEnvironment::from_interpreter(interpreter)
}

// Helper to check if a pypi package is installed.
fn is_pypi_package_installed(env: &PythonEnvironment, package_name: &str) -> bool {
    let site_packages = uv_installer::SitePackages::from_environment(env).unwrap();
    let found_packages = site_packages.get_packages(&PackageName::from_str(package_name).unwrap());
    !found_packages.is_empty()
}

// Helper to check if a conda package is installed.
async fn is_conda_package_installed(prefix_path: &Path, package_name: &str) -> bool {
    let conda_prefix = pixi_utils::prefix::Prefix::new(prefix_path.to_path_buf());
    conda_prefix
        .find_designated_package(&rattler_conda_types::PackageName::try_from(package_name).unwrap())
        .await
        .is_ok()
}

/// Test `pixi install --frozen --skip` functionality
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_frozen_skip() {
    setup_tracing();

    // Create a project with a local python dependency 'no-build-editable'
    // and a local conda dependency 'python_rich'
    let current_platform = Platform::current();
    let manifest = format!(
        r#"
        [workspace]
        channels = ["conda-forge"]
        description = "Add a short description here"
        name = "pyproject"
        platforms = ["{platform}"]
        preview = ["pixi-build"]
        version = "0.1.0"

        [dependencies]
        python = "*"
        python_rich = {{ path = "./python" }}

        [pypi-dependencies]
        no-build-editable = {{ path = "./no-build-editable" }}
        "#,
        platform = current_platform,
    );

    let pixi = PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project");

    let workspace_root = PathBuf::from(env!("CARGO_WORKSPACE_DIR"));

    fs_extra::dir::copy(
        workspace_root.join("docs/source_files/pixi_workspaces/pixi_build/python"),
        pixi.workspace_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();

    fs_extra::dir::copy(
        workspace_root.join("tests/data/satisfiability/no-build-editable"),
        pixi.workspace_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();

    pixi.update_lock_file().await.unwrap();

    // Check that neither 'python_rich' nor 'no-build-editable' are installed when skipped
    pixi.install()
        .with_frozen()
        .with_skipped(vec!["no-build-editable".into(), "python_rich".into()])
        .await
        .unwrap();

    let prefix_path = pixi.default_env_path().unwrap();
    let cache = uv_cache::Cache::temp().unwrap();
    let env = create_uv_environment(&prefix_path, &cache);

    assert!(!is_pypi_package_installed(&env, "no-build-editable"));
    assert!(!is_conda_package_installed(&prefix_path, "python_rich").await);

    // Check that 'no-build-editable' and 'python_rich' are installed after a followup normal install
    pixi.install().with_frozen().await.unwrap();

    assert!(is_pypi_package_installed(&env, "no-build-editable"));
    assert!(is_conda_package_installed(&prefix_path, "python_rich").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn pypi_reinstall_python() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.11").await.unwrap();

    // Add flask from pypi
    pixi.add("flask")
        .with_install(true)
        .set_type(pixi_core::DependencyType::PypiDependency)
        .await
        .unwrap();
    assert!(pixi.lock_file().await.unwrap().contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
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
        consts::DEFAULT_ENVIRONMENT_NAME,
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
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
// Check if we add and remove a pypi package that the site-packages is cleared
async fn pypi_add_remove() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();

    // Add flask from pypi
    pixi.add("flask[dotenv]")
        .with_install(true)
        .set_type(pixi_core::DependencyType::PypiDependency)
        .await
        .unwrap();

    let prefix = pixi.default_env_path().unwrap();

    let cache = uv_cache::Cache::temp().unwrap();

    // Check if site-packages has entries
    let env = create_uv_environment(&prefix, &cache);
    let installed_311 = uv_installer::SitePackages::from_environment(&env).unwrap();
    assert!(installed_311.iter().count() > 0);

    pixi.remove("flask[dotenv]")
        .set_type(pixi_core::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    let installed_311 = uv_installer::SitePackages::from_environment(&env).unwrap();
    assert!(installed_311.iter().count() == 0);
}

#[tokio::test]
async fn test_channels_changed() {
    setup_tracing();

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
    let lock_file = pixi.update_lock_file().await.unwrap();
    assert!(lock_file.contains_match_spec(consts::DEFAULT_ENVIRONMENT_NAME, platform, "bar ==2"));

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
    let lock_file = pixi.update_lock_file().await.unwrap();
    assert!(lock_file.contains_match_spec(consts::DEFAULT_ENVIRONMENT_NAME, platform, "bar ==1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn install_conda_meta_history() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    pixi.install().await.unwrap();

    let prefix = pixi.default_env_path().unwrap();
    let conda_meta_history_file = prefix.join("conda-meta/history");

    assert!(conda_meta_history_file.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn minimal_lockfile_update_pypi() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();

    // Add pypi dependencies which are not the latest options
    pixi.add_multiple(vec!["uvicorn==0.28.0", "click==7.1.2"])
        .set_type(pixi_core::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Check the locked click dependencies
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        pep508_rs::Requirement::from_str("click==7.1.2").unwrap()
    ));

    // Widening the click version to allow for the latest version
    pixi.add_multiple(vec!["uvicorn==0.29.0", "click"])
        .set_type(pixi_core::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // `click` should not be updated to a higher version.
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        pep508_rs::Requirement::from_str("click>7.1.2").unwrap()
    ));
}

/// Create a test that installs a package with pixi
/// change the installer and see if it does not touch the package
/// then change the installer back and see if it reinstalls the package
/// with a new version
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_installer_name() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(true).await.unwrap();
    pixi.add("click==8.0.0")
        .set_type(pixi_core::DependencyType::PypiDependency)
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
    let installer = fs_err::read_to_string(installer).unwrap();
    assert_eq!(installer, consts::PIXI_UV_INSTALLER);

    // Write a new installer name to the INSTALLER file
    // so that we fake that it is not installed by pixi
    fs_err::write(dist_info.join("INSTALLER"), "not-pixi").unwrap();
    pixi.remove("click==8.0.0")
        .with_install(true)
        .set_type(pixi_core::DependencyType::PypiDependency)
        .await
        .unwrap();

    // dist info folder should still exists
    // and should have the old installer name
    // we know that pixi did not touch the package
    assert!(dist_info.exists());
    let installer = dist_info.join("INSTALLER");
    let installer = fs_err::read_to_string(installer).unwrap();
    assert_eq!(installer, "not-pixi");

    // re-manage the package by adding it, this should cause a reinstall
    pixi.add("click==8.0.0")
        .set_type(pixi_core::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();
    let installer = dist_info.join("INSTALLER");
    let installer = fs_err::read_to_string(installer).unwrap();
    assert_eq!(installer, consts::PIXI_UV_INSTALLER);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
/// Test full prefix install for an old lock file to see if it still works.
/// Makes sure the lockfile isn't touched and the environment is still
/// installed.
async fn test_old_lock_install() {
    setup_tracing();

    let workspace_root = PathBuf::from(env!("CARGO_WORKSPACE_DIR"));
    let lock_str = fs_err::read_to_string(
        workspace_root.join("tests/data/satisfiability/old_lock_file/pixi.lock"),
    )
    .unwrap();
    let project = Workspace::from_path(
        &workspace_root.join("tests/data/satisfiability/old_lock_file/pyproject.toml"),
    )
    .unwrap();
    pixi_core::environment::get_update_lock_file_and_prefix(
        &project.default_environment(),
        UpdateMode::Revalidate,
        UpdateLockFileOptions {
            lock_file_usage: LockFileUsage::Update,
            no_install: false,
            ..Default::default()
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        lock_str,
        fs_err::read_to_string(
            workspace_root.join("tests/data/satisfiability/old_lock_file/pixi.lock")
        )
        .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_no_build_isolation() {
    setup_tracing();

    let current_platform = Platform::current();
    let setup_py = r#"
from setuptools import setup, find_packages
# custom import
import boltons
setup(
    name="my-pkg",
    version="0.1.0",
    author="Your Name",
    author_email="your.email@example.com",
    description="A brief description of your package",
    url="https://github.com/yourusername/your-repo",
    packages=find_packages(),  # Automatically find packages in your project
    classifiers=[
        "Programming Language :: Python :: 3",
        "License :: OSI Approved :: MIT License",
        "Operating System :: OS Independent",
    ],
    python_requires=">=3.6",
    install_requires=[
    ],
    entry_points={
        'console_scripts': [
            'your_command=your_package.module:main_function',
        ],
    },
)
    "#;

    let manifest = format!(
        r#"
    [project]
    name = "no-build-isolation"
    channels = ["https://prefix.dev/conda-forge"]
    platforms = ["{platform}"]

    [pypi-options]
    no-build-isolation = ["my-pkg"]

    [dependencies]
    python = "3.12.*"
    setuptools = ">=72,<73"
    boltons = ">=24,<25"

    [pypi-dependencies.my-pkg]
    path = "./my-pkg"
    "#,
        platform = current_platform,
    );

    let pixi = PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project");

    let project_path = pixi.workspace_path();
    // Write setup.py to a my-pkg folder
    let my_pkg = project_path.join("my-pkg");
    fs_err::create_dir_all(&my_pkg).unwrap();
    fs_err::write(my_pkg.join("setup.py"), setup_py).unwrap();

    let has_pkg = pixi
        .workspace()
        .unwrap()
        .default_environment()
        .pypi_options()
        .no_build_isolation
        .contains(&"my-pkg".parse().unwrap());

    assert!(has_pkg, "my-pkg is not in no-build-isolation list");
    pixi.install().await.expect("cannot install project");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_no_build_isolation_with_dependencies() {
    setup_tracing();

    let current_platform = Platform::current();

    // Create pyproject.toml for package-tdjager (will be installed with build isolation)
    let pyproject_toml_a = r#"
[build-system]
requires = ["setuptools>=45", "wheel"]
build-backend = "setuptools.build_meta"

[project]
name = "package-tdjager"
version = "0.1.0"
description = "Package Tim de Jager for testing build isolation"
authors = [{name = "Test Author"}]
requires-python = ">=3.6"
dependencies = []
    "#;

    // Create pyproject.toml for package-b (will be installed WITHOUT build isolation)
    // package-b depends on package-tdjager at build time
    let pyproject_toml_b = r#"
[build-system]
requires = ["setuptools>=45", "wheel", "package-tdjager"]
build-backend = "setuptools.build_meta"

[project]
name = "package-b"
version = "0.1.0"
description = "Package B that depends on Package Tim de Jager during build"
authors = [{name = "Test Author"}]
requires-python = ">=3.6"
dependencies = []
    "#;

    let manifest = format!(
        r#"
        [project]
        name = "no-build-isolation-deps"
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["{platform}"]

        [pypi-options]
        no-build-isolation = ["package-b"]

        [dependencies]
        python = "3.12.*"
        setuptools = ">=72,<73"

        [pypi-dependencies.package-b]
        path = "./package-b"

        [pypi-dependencies.package-tdjager]
        path = "./package-tdjager"

        "#,
        platform = current_platform,
    );

    let pixi = PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project");

    let project_path = pixi.workspace_path();

    // Create package-tdjager directory and pyproject.toml
    let package_a_dir = project_path.join("package-tdjager");
    fs_err::create_dir_all(&package_a_dir).unwrap();
    fs_err::write(package_a_dir.join("pyproject.toml"), pyproject_toml_a).unwrap();

    // Create empty __init__.py for package-tdjager
    let package_a_pkg_dir = package_a_dir.join("package_tdjager");
    fs_err::create_dir_all(&package_a_pkg_dir).unwrap();
    fs_err::write(package_a_pkg_dir.join("__init__.py"), "").unwrap();

    // Create setup.py for package-b that imports package-tdjager at build time
    let setup_py_b = r#"
from setuptools import setup

import package_tdjager

setup(
    name="package-b",
    version="0.1.0",
    description="Package B that depends on Package Tim de Jager during build",
    author="Test Author",
    python_requires=">=3.6",
    install_requires=[],
    setup_requires=["package-tdjager"],
)
    "#;

    // Create package-b directory, setup.py and pyproject.toml
    let package_b_dir = project_path.join("package-b");
    fs_err::create_dir_all(&package_b_dir).unwrap();
    fs_err::write(package_b_dir.join("pyproject.toml"), pyproject_toml_b).unwrap();
    fs_err::write(package_b_dir.join("setup.py"), setup_py_b).unwrap();

    // Create empty __init__.py for package-b
    let package_b_pkg_dir = package_b_dir.join("package_b");
    fs_err::create_dir_all(&package_b_pkg_dir).unwrap();
    fs_err::write(package_b_pkg_dir.join("__init__.py"), "").unwrap();

    // This should succeed with the 3-step installation:
    // 1. Conda packages (python, setuptools)
    // 2. PyPI packages with build isolation (package-tdjager) - batch
    // 3. PyPI packages without build isolation (package-b) - one by one
    // The key test: package-b should be able to import package-tdjager during its build
    pixi.install()
        .await
        .expect("cannot install project with no-build-isolation dependencies");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_setuptools_override_failure() {
    setup_tracing();

    // This was causing issues like: https://github.com/prefix-dev/pixi/issues/1686
    let manifest = format!(
        r#"
        [project]
        channels = ["https://prefix.dev/conda-forge"]
        name = "pixi-source-problem"
        platforms = ["{platform}"]
        exclude-newer = "2024-08-29"

        [dependencies]
        pip = ">=24.0,<25"
        python = "<3.13"
        # we need to pin them because xalas start to fail with newer versions
        ninja = "*"
        cmake = "<4"

        [pypi-options]
        no-build-isolation = ["xatlas"]

        # The transitive dependencies of viser were causing issues
        [pypi-dependencies]
        viser = "==0.2.7"
        "#,
        platform = Platform::current()
    );
    let pixi = PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project");
    pixi.install().await.expect("cannot install project");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_many_linux_wheel_tag() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    #[cfg(not(target_os = "linux"))]
    pixi.init_with_platforms(vec![
        Platform::current().to_string(),
        "linux-64".to_string(),
    ])
    .await
    .unwrap();
    #[cfg(target_os = "linux")]
    pixi.init().await.unwrap();

    pixi.add("python==3.12.*").await.unwrap();
    // We know that this package has many linux wheel tags for this version
    pixi.add("gmsh==4.13.1")
        .set_type(pixi_core::DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_ensure_gitignore_file_creation() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    let gitignore_path = pixi.workspace().unwrap().pixi_dir().join(".gitignore");
    assert!(
        !gitignore_path.exists(),
        ".pixi/.gitignore file should not exist"
    );

    // Check that .gitignore is created after the first install and contains '*'
    pixi.install().await.unwrap();
    assert!(
        gitignore_path.exists(),
        ".pixi/.gitignore file was not created"
    );
    let contents = tokio_fs::read_to_string(&gitignore_path).await.unwrap();
    assert_eq!(
        contents, "*\n!config.toml\n",
        ".pixi/.gitignore file does not contain the expected content"
    );

    // Modify the .gitignore file and check that it is preserved after reinstall
    tokio::fs::write(&gitignore_path, "*\nsome_file\n")
        .await
        .unwrap();
    pixi.install().await.unwrap();
    let contents = tokio_fs::read_to_string(&gitignore_path).await.unwrap();
    assert_eq!(
        contents, "*\nsome_file\n",
        ".pixi/.gitignore file does not contain the expected content"
    );

    // Remove the .gitignore file and check that it is recreated
    tokio::fs::remove_file(&gitignore_path).await.unwrap();
    assert!(
        !gitignore_path.exists(),
        ".pixi/.gitignore file should not exist"
    );
    pixi.install().await.unwrap();
    assert!(
        gitignore_path.exists(),
        ".pixi/.gitignore file was not recreated"
    );
    let contents = tokio_fs::read_to_string(&gitignore_path).await.unwrap();
    assert_eq!(
        contents, "*\n!config.toml\n",
        ".pixi/.gitignore file does not contain the expected content"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn pypi_prefix_is_not_created_when_whl() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    // Add and update lockfile with this version of python
    pixi.add("python==3.11").with_install(false).await.unwrap();

    // Add pypi dependency that is a wheel
    pixi.add_multiple(vec!["boltons==24.1.0"])
        .set_type(pixi_core::DependencyType::PypiDependency)
        // we don't want to install the package
        // we just want to check that the prefix is not created
        .with_install(false)
        .await
        .unwrap();

    // Check the locked boltons dependencies
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        pep508_rs::Requirement::from_str("boltons==24.1.0").unwrap()
    ));

    let default_env_prefix = pixi.default_env_path().unwrap();

    // Check that the prefix is not created
    assert!(!default_env_prefix.exists());
}

/// This test checks that the override of a conda package is correctly done per
/// platform. There have been issues in the past that the wrong repodata was
/// used for the override. What this test does is recreate this situation by
/// adding a conda package that is only available on linux and then adding a
/// PyPI dependency on the same package for both linux and osxarm64.
/// This should result in the PyPI package being overridden on linux and not on
/// osxarm64.
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn conda_pypi_override_correct_per_platform() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init_with_platforms(vec![
        Platform::OsxArm64.to_string(),
        Platform::Linux64.to_string(),
        Platform::Win64.to_string(),
        Platform::Osx64.to_string(),
    ])
    .await
    .unwrap();
    pixi.add("python==3.12").with_install(false).await.unwrap();

    // Add a conda package that is only available on linux
    pixi.add("boltons")
        .with_platform(Platform::Linux64)
        .with_install(false)
        .await
        .unwrap();

    // Add a PyPI dependency on boltons as well
    pixi.add("boltons")
        .set_pypi(true)
        .with_install(false)
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    // Check that the conda package is only available on linux
    assert!(lock.contains_conda_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        "boltons"
    ));
    // Sanity check that the conda package is not available on osxarm64
    assert!(!lock.contains_conda_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::OsxArm64,
        "boltons"
    ));
    // Check that the PyPI package is available on osxarm64 only
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::OsxArm64,
        pep508_rs::Requirement::from_str("boltons").unwrap(),
    ));
    assert!(!lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        pep508_rs::Requirement::from_str("boltons").unwrap(),
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]

async fn test_multiple_prefix_update() {
    setup_tracing();

    let current_platform = Platform::current();
    let virtual_packages = VirtualPackages::detect(&VirtualPackageOverrides::default())
        .unwrap()
        .into_generic_virtual_packages()
        .collect();

    let pixi = PixiControl::from_manifest(
        format!(
            r#"
    [project]
    name = "test-channel-change"
    channels = ["https://prefix.dev/conda-forge"]
    platforms = ["{platform}"]
    "#,
            platform = current_platform
        )
        .as_str(),
    )
    .unwrap();

    let project = pixi.workspace().unwrap();

    let (url, sha256, md5) = if cfg!(target_os = "windows") {
        (
            "https://repo.prefix.dev/conda-forge/win-64/python-3.13.1-h071d269_105_cp313.conda",
            "de3bb832ff3982c993c6af15e6c45bb647159f25329caceed6f73fd4769c7628",
            "3ddb0531ecfb2e7274d471203e053d78",
        )
    } else if cfg!(target_os = "macos") {
        (
            "https://repo.prefix.dev/conda-forge/osx-64/python-3.13.1-h2334245_105_cp313.conda",
            "a9d224fa69c8b58c8112997f03988de569504c36ba619a08144c47512219e5ad",
            "c3318c58d14fefd755852e989c991556",
        )
    } else {
        (
            "https://repo.prefix.dev/conda-forge/linux-64/python-3.13.1-ha99a958_105_cp313.conda",
            "d3eb7d0820cf0189103bba1e60e242ffc15fd2f727640ac3a10394b27adf3cca",
            "34945787453ee52a8f8271c1d19af1e8",
        )
    };

    let python_package = Package::build("python", "3.13.1")
        .with_hashes(sha256, md5)
        .finish();

    let python_repo_data_record = RepoDataRecord {
        package_record: python_package.package_record,
        file_name: "python".to_owned(),
        url: Url::parse(url).unwrap(),
        channel: Some("https://repo.prefix.dev/conda-forge/".to_owned()),
    };

    let wheel_package = Package::build("wheel", "0.45.1")
        .with_hashes(
            "1b34021e815ff89a4d902d879c3bd2040bc1bd6169b32e9427497fa05c55f1ce",
            "75cb7132eb58d97896e173ef12ac9986",
        )
        .finish();

    let wheel_repo_data_record = RepoDataRecord {
        package_record: wheel_package.package_record,
        file_name: "wheel".to_owned(),
        url: Url::parse(
            "https://repo.prefix.dev/conda-forge/noarch/wheel-0.45.1-pyhd8ed1ab_1.conda",
        )
        .unwrap(),
        channel: Some("https://repo.prefix.dev/conda-forge/".to_owned()),
    };

    let group = GroupedEnvironment::from(project.default_environment().clone());

    // Normally in pixi, the RAYON_INITIALIZE is lazily initialized by the reporter
    // associated with the command dispatcher.
    LazyLock::force(&RAYON_INITIALIZE);

    let channels = group
        .channel_urls(&group.workspace().channel_config())
        .unwrap();
    let name = group.name();
    let prefix = group.prefix();

    let command_dispatcher = project.command_dispatcher_builder().unwrap().finish();

    let conda_prefix_updater = CondaPrefixUpdater::new(
        channels,
        group.workspace().channel_config(),
        name,
        prefix,
        current_platform,
        virtual_packages,
        group.workspace().variants(current_platform),
        command_dispatcher,
    );

    let pixi_records = Vec::from([
        PixiRecord::Binary(wheel_repo_data_record),
        PixiRecord::Binary(python_repo_data_record),
    ]);

    let mut sets = JoinSet::new();

    // spawn multiple tokio tasks to update the prefix
    for _ in 0..4 {
        let pixi_records = pixi_records.clone();
        // tasks.push(conda_prefix_updater.update(pixi_records));
        let updater = conda_prefix_updater.clone();
        sets.spawn(async move { updater.update(pixi_records, None, None).await.cloned() });
    }

    let mut first_modified = None;

    while let Some(result) = sets.join_next().await {
        let prefix_updated = result.unwrap().unwrap();

        let prefix = prefix_updated.prefix.root();

        assert_eq!(
            prefix_updated
                .prefix
                .find_installed_packages()
                .unwrap()
                .len(),
            2
        );

        let prefix_metadata = fs::metadata(prefix).await.unwrap();

        let first_modified_date = first_modified.get_or_insert(prefix_metadata.modified().unwrap());

        // verify that the prefix was updated only once, meaning that we instantiated
        // prefix only once
        assert_eq!(*first_modified_date, prefix_metadata.modified().unwrap());
    }
}

/// Should download a package from an S3 bucket and install it
#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn install_s3() {
    setup_tracing();

    let r2_access_key_id = std::env::var("PIXI_TEST_R2_ACCESS_KEY_ID").ok();
    let r2_secret_access_key = std::env::var("PIXI_TEST_R2_SECRET_ACCESS_KEY").ok();
    if r2_access_key_id.is_none()
        || r2_access_key_id.clone().unwrap().is_empty()
        || r2_secret_access_key.is_none()
        || r2_secret_access_key.clone().unwrap().is_empty()
    {
        eprintln!(
            "Skipping test as PIXI_TEST_R2_ACCESS_KEY_ID or PIXI_TEST_R2_SECRET_ACCESS_KEY is not set"
        );
        return;
    }

    let r2_access_key_id = r2_access_key_id.unwrap();
    let r2_secret_access_key = r2_secret_access_key.unwrap();

    let credentials = format!(
        r#"
    {{
        "s3://rattler-s3-testing/channel": {{
            "S3Credentials": {{
                "access_key_id": "{}",
                "secret_access_key": "{}"
            }}
        }}
    }}
    "#,
        r2_access_key_id, r2_secret_access_key
    );
    let temp_dir = tempdir().unwrap();
    let credentials_path = temp_dir.path().join("credentials.json");
    let mut file = File::create(credentials_path.clone()).unwrap();
    file.write_all(credentials.as_bytes()).unwrap();

    let manifest = format!(
        r#"
    [project]
    name = "s3-test"
    channels = ["s3://rattler-s3-testing/channel", "https://prefix.dev/conda-forge"]
    platforms = ["{platform}"]

    [project.s3-options.rattler-s3-testing]
    endpoint-url = "https://e1a7cde76f1780ec06bac859036dbaf7.eu.r2.cloudflarestorage.com"
    region = "auto"
    force-path-style = true

    [dependencies]
    my-webserver = {{ version = "0.1.0", build = "pyh4616a5c_0" }}
    "#,
        platform = Platform::current(),
    );

    let pixi = PixiControl::from_manifest(&manifest).expect("cannot instantiate pixi project");

    temp_env::async_with_vars(
        [(
            "RATTLER_AUTH_FILE",
            Some(credentials_path.to_str().unwrap()),
        )],
        async {
            pixi.install().await.unwrap();
        },
    )
    .await;

    // Test for existence of conda-meta/my-webserver-0.1.0-pyh4616a5c_0.json file
    assert!(
        pixi.default_env_path()
            .unwrap()
            .join("conda-meta")
            .join("my-webserver-0.1.0-pyh4616a5c_0.json")
            .exists()
    );
}

#[tokio::test]
async fn test_exclude_newer() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Create a channel with two packages with different timestamps
    package_database.add_package(
        Package::build("foo", "1")
            .with_timestamp("2010-12-02T02:07:43Z".parse().unwrap())
            .finish(),
    );
    package_database.add_package(
        Package::build("foo", "2")
            .with_timestamp("2020-12-02T07:00:00Z".parse().unwrap())
            .finish(),
    );

    // Create a project that includes the package `foo` with a version specifier
    // without an exclude-newer.
    let channel = package_database.into_channel().await.unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [workspace]
    name = "test-channel-change"
    channels = ["{channel_a}"]
    platforms = ["{platform}"]

    [dependencies]
    foo = "*"
    "#,
        channel_a = channel.url(),
        platform = Platform::current()
    ))
    .unwrap();

    // Create the lock-file
    pixi.lock().await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==2"
    ));

    // Update the manifest with the exclude-never field
    pixi.update_manifest(&format!(
        r#"
    [project]
    name = "test-channel-change"
    channels = ["{channel_a}"]
    platforms = ["{platform}"]
    exclude-newer = "2015-12-02T02:07:43Z"

    [dependencies]
    foo = "*"
    "#,
        channel_a = channel.url(),
        platform = Platform::current()
    ))
    .unwrap();

    // Relock and check that an older version of the package is selected
    pixi.lock().await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==1"
    ));
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_exclude_newer_pypi() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [workspace]
    name = "test-channel-change"
    channels = ["https://prefix.dev/conda-forge"]
    platforms = ["{platform}"]
    exclude-newer = "2020-12-02"

    [dependencies]
    python = "*"

    [pypi-dependencies]
    boltons = "*"
    "#,
        platform = Platform::current()
    ))
    .unwrap();

    // Create the lock-file
    pixi.lock().await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "boltons ==20.2.1".parse().unwrap()
    ));
}
