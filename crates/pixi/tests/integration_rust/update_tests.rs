use std::str::FromStr;

use pixi_consts::consts;
use rattler_conda_types::Platform;
use rattler_lock::LockFile;
use tempfile::TempDir;

use crate::common::{GitRepoFixture, LockFileExt, PixiControl};
use crate::setup_tracing;
use pixi_test_utils::{MockRepoData, Package};

#[tokio::test]
async fn test_update() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

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

    let mut package_database = MockRepoData::default();

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

    // Create local package database with Python
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    package_database.add_package(
        Package::build("python", "3.12.1")
            .with_subdir(Platform::current())
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two commits
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current()])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a git pypi dependency using local fixture
    pixi.add_pypi(&format!("minimal-package @ {}", fixture.url))
        .await
        .unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let pkg = lock
        .get_pypi_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "minimal-package",
        )
        .unwrap();

    let pkg_version = pkg.as_pypi().unwrap().0.version.to_string();

    let mut lock_file_str = lock.render_to_string().unwrap();

    // git url should have a fragment
    let fragment = pkg
        .as_pypi()
        .unwrap()
        .0
        .location
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");

    // Modify this fragment to simulate an older commit (first commit of fixture)
    lock_file_str = lock_file_str.replace(fragment, fixture.first_commit());

    lock_file_str = lock_file_str.replace(&pkg_version, "0.1.0");

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
            "minimal-package",
        )
        .unwrap();
    let new_fragment = url_or_path
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");
    assert_eq!(
        fixture.first_commit(),
        new_fragment,
        "expected git pypi package to not be updated when updating conda packages"
    );
}

#[tokio::test]
async fn test_update_conda_package_doesnt_update_git_pypi_pinned() {
    setup_tracing();

    // Create local package database with Python
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    package_database.add_package(
        Package::build("python", "3.12.1")
            .with_subdir(Platform::current())
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two commits
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current()])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a `pinned` git pypi dependency using local fixture (pinned to first commit)
    pixi.add_pypi(&format!(
        "minimal-package @ {}@{}",
        fixture.url,
        fixture.first_commit()
    ))
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

    // Create local package database with Python
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two commits
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current()])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a `pinned` git pypi dependency using local fixture (pinned to first commit)
    pixi.add_pypi(&format!(
        "minimal-package @ {}@{}",
        fixture.url,
        fixture.first_commit()
    ))
    .await
    .unwrap();

    // now remove the pin to allow updates
    let manifest_txt = tokio::fs::read_to_string(pixi.manifest_path())
        .await
        .unwrap();

    // remove the pin from the dependency
    let new_manifest_txt =
        manifest_txt.replace(&format!(", rev = \"{}\"", fixture.first_commit()), "");

    tokio::fs::write(pixi.manifest_path(), new_manifest_txt)
        .await
        .unwrap();

    // run pixi update to re-lock
    pixi.update().with_package("minimal-package").await.unwrap();

    // Get the created lock-file
    let lock = pixi.lock_file().await.unwrap();

    // find the package
    let pkg = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current(),
            "minimal-package",
        )
        .unwrap();

    let pkg_fragment = pkg
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");

    // We expect the fragment to be the latest commit, not the first
    assert_eq!(pkg_fragment, fixture.latest_commit());
}
