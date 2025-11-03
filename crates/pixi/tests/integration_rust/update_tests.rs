use std::str::FromStr;

use pixi_consts::consts;
use rattler_conda_types::Platform;
use rattler_lock::LockFile;
use tempfile::TempDir;

use crate::common::{
    LockFileExt, PixiControl,
    package_database::{Package, PackageDatabase},
};
use crate::setup_tracing;

#[tokio::test]
async fn test_update() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add a package
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("foo", "1").finish());

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

    // Add a dependency on `bar`
    pixi.add("bar <=2").await.unwrap();
    pixi.add("foo <=2").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "bar ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==1"
    ));

    // Add version 2 and 3 of `bar`. Version 3 should never be selected.
    package_database.add_package(Package::build("bar", "2").finish());
    package_database.add_package(Package::build("bar", "3").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database.add_package(Package::build("foo", "3").finish());
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Run the update command to update all the packages
    pixi.update().await.unwrap();

    // Reload the lock-file and check if the new version of `bar` still matches the
    // spec and has been updated.
    let lock = pixi.lock_file().await.unwrap();
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we updated the lock-file"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "bar ==2"
        ),
        "expected `bar` to be on version 2 because we updated the lock-file"
    );
}

#[tokio::test]
async fn test_update_single_package() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add packages
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("foo", "1").finish());

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

    // Add a dependency on `bar`
    pixi.add("bar <=2").await.unwrap();
    pixi.add("foo <=2").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "bar ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "foo ==1"
    ));

    // Add version 2 and 3 of `bar`. Version 3 should never be selected.
    package_database.add_package(Package::build("bar", "2").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    // Run the update command to update a single package
    pixi.update().with_package("foo").await.unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we updated it"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "bar ==1"
        ),
        "expected `bar` to be on version 1 because only foo should be updated"
    );
}

#[tokio::test]
async fn test_update_conda_package_doesnt_update_git_pypi() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    // Create a new project using our package database.
    pixi.init()
        .with_platforms(vec![Platform::current()])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a git pypi dependency on `tqdm`
    pixi.add_pypi("tqdm @ git+https://github.com/tqdm/tqdm.git")
        .await
        .unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let tqmd_package = lock
        .get_pypi_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "tqdm",
        )
        .unwrap();

    let tqmd_version = tqmd_package.as_pypi().unwrap().0.version.to_string();

    let mut lock_file_str = lock.render_to_string().unwrap();

    // git url should have a fragment
    let fragment = tqmd_package
        .as_pypi()
        .unwrap()
        .0
        .location
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");

    // and modify this fragment to simulate an older commit
    let older_commit = "a2d5f1c9d1cbdbcf56f52dc4365ea4124e3e33f7";
    lock_file_str = lock_file_str.replace(fragment, older_commit);

    lock_file_str = lock_file_str.replace(&tqmd_version, "4.67.1.dev5+ga2d5f1c9d");

    let lockfile = LockFile::from_str(&lock_file_str).unwrap();

    lockfile.to_path(&workspace.lock_file_path()).unwrap();

    // now run the update command to update conda packages
    // which will invalidate also pypi packages
    pixi.update().with_package("python").await.unwrap();

    // Get the re-locked lock-file
    let lock = pixi.lock_file().await.unwrap();

    let url_or_path = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "tqdm",
        )
        .unwrap();
    let new_fragment = url_or_path
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");
    assert_eq!(
        older_commit, new_fragment,
        "expected git pypi package to not be updated when updating conda packages"
    );
}

#[tokio::test]
async fn test_update_conda_package_doesnt_update_git_pypi_pinned() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    // Create a new project using our package database.
    pixi.init()
        .with_platforms(vec![Platform::current()])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a `pinned` git pypi dependency on `tqdm`
    // this should not trigger an update
    pixi.add_pypi(
        "tqdm @ git+https://github.com/tqdm/tqdm.git@cac7150d7c8a650c7e76004cd7f8643990932c7f",
    )
    .await
    .unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();

    // previous lockfile
    let previous_lockfile_str = lock.render_to_string().unwrap();

    // now run the update command to update conda packages
    // which should not trigger any update for the pinned pypi package
    pixi.update().with_package("python").await.unwrap();

    // Get the re-locked lock-file
    let lock = pixi.lock_file().await.unwrap();

    let new_lockfile_str = lock.render_to_string().unwrap();

    assert_eq!(
        previous_lockfile_str, new_lockfile_str,
        "expected git pypi package to not be updated when updating conda packages"
    );
}

#[tokio::test]
async fn test_update_git_pypi_when_requested() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    // Create a new project using our package database.
    pixi.init()
        .with_platforms(vec![Platform::current()])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a `pinned` git pypi dependency on `tqdm`
    pixi.add_pypi(
        "tqdm @ git+https://github.com/tqdm/tqdm.git@cac7150d7c8a650c7e76004cd7f8643990932c7f",
    )
    .await
    .unwrap();

    // now remove the pin to allow updates
    let manifest_txt = tokio::fs::read_to_string(pixi.manifest_path())
        .await
        .unwrap();

    // remove the pin from the dependency
    let new_manifest_txt =
        manifest_txt.replace(", rev = \"cac7150d7c8a650c7e76004cd7f8643990932c7f\"", "");

    tokio::fs::write(pixi.manifest_path(), new_manifest_txt)
        .await
        .unwrap();

    // run pixi update to re-lock
    pixi.update().with_package("tqdm").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();

    // find the tqdm package
    let tqmd_package = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "tqdm",
        )
        .unwrap();

    let tqdm_fragment = tqmd_package
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");

    // we expect the fragment to be different than the previous pinned one
    assert_ne!(tqdm_fragment, "cac7150d7c8a650c7e76004cd7f8643990932c7f");
}
