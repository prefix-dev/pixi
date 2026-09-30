use crate::common::{LockFileExt, PixiControl};
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Platform;
use tempfile::TempDir;

/// Test that `pixi lock --dry-run` doesn't modify the lock file on disk
#[tokio::test]
async fn test_lock_dry_run_doesnt_modify_lock_file() {
    // Create a mock package database
    let mut package_database = MockRepoData::default();

    // Add mock packages
    package_database.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    package_database.add_package(
        Package::build("numpy", "1.24.0")
            .with_subdir(Platform::current())
            .finish(),
    );

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Create a new pixi project using our local channel
    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a dependency to create an initial lock file
    pixi.add("python").await.unwrap();

    // Get the original lock file
    let original_lock_file = pixi.lock_file().await.unwrap();
    let platform = Platform::current();

    // Verify python is in the original lock file
    assert!(
        original_lock_file.contains_conda_package("default", platform, "python"),
        "python should be in the initial lock file"
    );

    // Add another dependency to the manifest without updating the lock file
    let manifest_content = pixi.manifest_contents().unwrap();
    let updated_manifest =
        manifest_content.replace("[dependencies]", "[dependencies]\nnumpy = \"*\"");
    pixi.update_manifest(&updated_manifest).unwrap();

    // Run `pixi lock --dry-run`
    pixi.lock().with_dry_run(true).await.unwrap();

    // Verify the lock file was NOT modified
    let lock_after_dry_run = pixi.lock_file().await.unwrap();

    assert!(
        lock_after_dry_run.contains_conda_package("default", platform, "python"),
        "python should still be in lock file after --dry-run"
    );

    assert!(
        !lock_after_dry_run.contains_conda_package("default", platform, "numpy"),
        "numpy should NOT be in lock file after --dry-run"
    );

    // Now run without --dry-run to actually update the lock file
    pixi.lock().await.unwrap();

    // Verify the lock file WAS modified this time
    let lock_after_normal = pixi.lock_file().await.unwrap();

    assert!(
        lock_after_normal.contains_conda_package("default", platform, "python"),
        "python should still be in lock file"
    );

    assert!(
        lock_after_normal.contains_conda_package("default", platform, "numpy"),
        "numpy should NOW be in lock file after normal lock"
    );
}

/// Test that `pixi lock --dry-run` implies `--no-install`
#[tokio::test]
async fn test_lock_dry_run_implies_no_install() {
    // Create a mock package database
    let mut package_database = MockRepoData::default();

    // Add mock packages
    package_database.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    package_database.add_package(
        Package::build("numpy", "1.24.0")
            .with_subdir(Platform::current())
            .finish(),
    );

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Create a new pixi project using our local channel
    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Add a dependency
    pixi.add("python").await.unwrap();

    // Get the environment path
    let env_path = pixi.default_env_path().unwrap();

    // Remove the environment directory if it exists
    if env_path.exists() {
        fs_err::remove_dir_all(&env_path).unwrap();
    }

    // Add another dependency to manifest
    let manifest_content = pixi.manifest_contents().unwrap();
    let updated_manifest =
        manifest_content.replace("[dependencies]", "[dependencies]\nnumpy = \"*\"");
    pixi.update_manifest(&updated_manifest).unwrap();

    // Run `pixi lock --dry-run`
    pixi.lock().with_dry_run(true).await.unwrap();

    // Environment should NOT have been created
    assert!(
        !env_path.exists(),
        "Environment should not be created with --dry-run"
    );
}

/// Test that a feature with extra platforms does not leak into the default environment (prefix-dev/pixi#6770)
#[tokio::test]
async fn test_feature_platforms_do_not_leak_into_default_environment() {
    let mut package_database = MockRepoData::default();

    // libgcc-ng is only available on linux-64
    package_database.add_package(
        Package::build("libgcc-ng", "12.0.0")
            .with_subdir(Platform::Linux64)
            .finish(),
    );

    // dev-pkg is available on both linux-64 and osx-arm64
    package_database.add_package(
        Package::build("dev-pkg", "1.0.0")
            .with_subdir(Platform::Linux64)
            .finish(),
    );
    package_database.add_package(
        Package::build("dev-pkg", "1.0.0")
            .with_subdir(Platform::OsxArm64)
            .finish(),
    );

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let channel_url = url::Url::from_directory_path(channel_dir.path()).unwrap();

    let pixi = PixiControl::new().unwrap();
    let manifest = format!(
        r#"
        [workspace]
        name = "test-leak"
        channels = ["{channel_url}"]
        platforms = ["linux-64"]

        [dependencies]
        libgcc-ng = "*"

        [feature.dev]
        platforms = ["linux-64", "osx-arm64"]

        [feature.dev.dependencies]
        dev-pkg = "*"

        [environments]
        dev = {{ features = ["dev"], no-default-feature = true }}
        "#
    );
    pixi.init().await.unwrap();
    pixi.update_manifest(&manifest).unwrap();

    // Lock must succeed because default only targets linux-64 where libgcc-ng exists,
    // and dev doesn't include libgcc-ng.
    pixi.lock().await.unwrap();

    let lock_file = pixi.lock_file().await.unwrap();
    assert!(
        lock_file.contains_conda_package("default", Platform::Linux64, "libgcc-ng"),
        "libgcc-ng should be in default environment for linux-64"
    );
    let default_env = lock_file.environment("default").unwrap();
    let linux_p = lock_file.platform("linux-64");
    assert!(
        linux_p.and_then(|p| default_env.packages(p)).is_some(),
        "default environment must have linux-64 packages"
    );
    let osx_p = lock_file.platform("osx-arm64");
    assert!(
        osx_p.and_then(|p| default_env.packages(p)).is_none(),
        "default environment must not have osx-arm64 packages"
    );

    assert!(
        lock_file.contains_conda_package("dev", Platform::Linux64, "dev-pkg"),
        "dev-pkg should be in dev environment for linux-64"
    );
    assert!(
        lock_file.contains_conda_package("dev", Platform::OsxArm64, "dev-pkg"),
        "dev-pkg should be in dev environment for osx-arm64"
    );
}
