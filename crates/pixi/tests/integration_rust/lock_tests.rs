use crate::common::{LockFileExt, PixiControl};
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Subdir;
use tempfile::TempDir;

/// Test that `pixi lock --dry-run` doesn't modify the lock file on disk
#[tokio::test]
async fn test_lock_dry_run_doesnt_modify_lock_file() {
    // Create a mock package database
    let mut package_database = MockRepoData::default();

    // Add mock packages
    package_database.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(Subdir::current().unwrap_or(Subdir::NoArch))
            .finish(),
    );
    package_database.add_package(
        Package::build("numpy", "1.24.0")
            .with_subdir(Subdir::current().unwrap_or(Subdir::NoArch))
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
    let platform = Subdir::current().unwrap_or(Subdir::NoArch);

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
            .with_subdir(Subdir::current().unwrap_or(Subdir::NoArch))
            .finish(),
    );
    package_database.add_package(
        Package::build("numpy", "1.24.0")
            .with_subdir(Subdir::current().unwrap_or(Subdir::NoArch))
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

/// `pixi lock --check` must never create a lockfile when one is missing (#7081).
#[tokio::test]
async fn test_lock_check_does_not_create_missing_lock_file() {
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(Subdir::current().unwrap_or(Subdir::NoArch))
            .finish(),
    );

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();

    // Ensure there is no lockfile on disk.
    let lock_path = pixi.workspace_path().join("pixi.lock");
    let _ = fs_err::remove_file(&lock_path);
    assert!(
        !lock_path.exists(),
        "precondition: pixi.lock must be absent"
    );

    // First --check should fail and must not create the lockfile.
    let err = pixi.lock().with_check(true).await.unwrap_err();
    assert!(
        err.to_string().contains("lock file not up-to-date")
            || format!("{err:?}").contains("lock file not up-to-date"),
        "unexpected error: {err:?}"
    );
    assert!(
        !lock_path.exists(),
        "pixi.lock must not be created by the first --check"
    );

    // Second --check must still fail the same way — proving the first run did not write a lock.
    let err = pixi.lock().with_check(true).await.unwrap_err();
    assert!(
        err.to_string().contains("lock file not up-to-date")
            || format!("{err:?}").contains("lock file not up-to-date"),
        "unexpected error: {err:?}"
    );
    assert!(
        !lock_path.exists(),
        "pixi.lock must not be created by the second --check"
    );
}

/// `pixi lock --constraint` re-solves only the environments whose locked
/// packages violate the constraint, keeps the other locked packages as
/// preferences, and never writes the constraint to the manifest.
#[tokio::test]
async fn test_lock_constraint_minimal_update() {
    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("baz", "1").finish());

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let channel = url::Url::from_file_path(channel_dir.path()).unwrap();
    let platform = Subdir::current().unwrap_or(Subdir::NoArch);
    let manifest = format!(
        r#"
    [workspace]
    name = "test-lock-constraint"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    [feature.a.dependencies]
    foo = "*"
    bar = "*"
    baz = "*"

    [feature.b.dependencies]
    bar = "*"

    [environments]
    a = {{ features = ["a"], no-default-feature = true }}
    b = {{ features = ["b"], no-default-feature = true }}
    "#
    );
    let pixi = PixiControl::from_manifest(&manifest).unwrap();
    pixi.lock().await.unwrap();

    // Publish newer versions. The fixed `foo` needs a newer `baz`.
    package_database.add_package(
        Package::build("foo", "2")
            .with_dependency("baz >=2")
            .finish(),
    );
    package_database.add_package(Package::build("foo", "3").finish());
    package_database.add_package(Package::build("bar", "2").finish());
    package_database.add_package(Package::build("baz", "2").finish());
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Without the constraint the lock file is up to date.
    pixi.lock().with_check(true).await.unwrap();

    // With the constraint the lock file is out of date, but `--check` must not
    // write anything.
    let err = pixi
        .lock()
        .with_check(true)
        .with_constraint("foo>=2,<3")
        .await
        .unwrap_err();
    assert!(
        format!("{err:?}").contains("lock file not up-to-date"),
        "unexpected error: {err:?}"
    );
    pixi.lock()
        .with_dry_run(true)
        .with_constraint("foo>=2,<3")
        .await
        .unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec("a", platform, "foo ==1"));

    pixi.lock().with_constraint("foo>=2,<3").await.unwrap();

    let lock = pixi.lock_file().await.unwrap();
    // The constrained package moved to the newest allowed version.
    assert!(lock.contains_match_spec("a", platform, "foo ==2"));
    // Its dependency was allowed to move, because locked packages are only
    // preferences.
    assert!(lock.contains_match_spec("a", platform, "baz ==2"));
    // Unrelated packages keep their locked version.
    assert!(lock.contains_match_spec("a", platform, "bar ==1"));
    // Environments without the package are untouched.
    assert!(lock.contains_match_spec("b", platform, "bar ==1"));
    assert!(!lock.contains_conda_package("b", platform, "foo"));

    // The constraint is not persisted in the manifest.
    assert_eq!(pixi.manifest_contents().unwrap(), manifest);

    // The updated lock file still satisfies the manifest and the constraint.
    pixi.lock().with_check(true).await.unwrap();
    pixi.lock()
        .with_check(true)
        .with_constraint("foo>=2")
        .await
        .unwrap();
}

/// A constraint that cannot be satisfied results in a solve error.
#[tokio::test]
async fn test_lock_constraint_unsatisfiable() {
    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());

    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();
    pixi.add("foo").await.unwrap();

    pixi.lock()
        .with_constraint("foo>=2")
        .await
        .expect_err("solving with an unsatisfiable constraint should fail");

    // Invalid match specs are rejected.
    let err = pixi.lock().with_constraint("foo >=>2").await.unwrap_err();
    assert!(
        format!("{err:?}").contains("failed to parse constraint"),
        "unexpected error: {err:?}"
    );
}
