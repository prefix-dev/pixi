use crate::common::{LockFileExt, PixiControl};
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Platform;
use tempfile::TempDir;

/// Test that `pixi lock --dry-run` doesn't modify the lock file on disk
#[tokio::test]
async fn test_lock_dry_run_doesnt_modify_lockfile() {
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
