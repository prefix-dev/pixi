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

    // Get the created lock file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current().expect("host platform"),
        "bar ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current().expect("host platform"),
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

    // Reload the lock file and check if the new version of `bar` still matches the
    // spec and has been updated.
    let lock = pixi.lock_file().await.unwrap();
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we updated the lock file"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "bar ==2"
        ),
        "expected `bar` to be on version 2 because we updated the lock file"
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

    // Get the created lock file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current().expect("host platform"),
        "bar ==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current().expect("host platform"),
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
            Platform::current().expect("host platform"),
            "foo ==2"
        ),
        "expected `foo` to be on version 2 because we updated it"
    );
    assert!(
        lock.contains_match_spec(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
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
            .with_subdir(Platform::current().expect("host platform"))
            .finish(),
    );
    package_database.add_package(
        Package::build("python", "3.12.1")
            .with_subdir(Platform::current().expect("host platform"))
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two commits
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current().expect("host platform")])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Add a git pypi dependency using local fixture
    pixi.add_pypi(&format!("minimal-package @ {}", fixture.url))
        .await
        .unwrap();

    // Get the created lock file
    let lock = pixi.lock_file().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let pkg = lock
        .get_pypi_package(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "minimal-package",
        )
        .unwrap();

    let pkg_version = pkg.as_pypi().unwrap().version_string();

    let mut lock_file_str = lock.render_to_string().unwrap();

    // git url should have a fragment
    let fragment = pkg
        .as_pypi()
        .unwrap()
        .location()
        .as_url()
        .unwrap()
        .fragment()
        .expect("expected git url to have a fragment");

    // Modify this fragment to simulate an older commit (first commit of fixture)
    lock_file_str = lock_file_str.replace(fragment, fixture.first_commit());

    lock_file_str = lock_file_str.replace(&pkg_version, "0.1.0");

    let lock_file = LockFile::from_str_with_base_directory(&lock_file_str, None).unwrap();

    lock_file.to_path(&workspace.lock_file_path()).unwrap();

    // now run the update command to update conda packages
    // which will invalidate also pypi packages
    pixi.update().with_package("python").await.unwrap();

    // Get the re-locked lock file
    let lock = pixi.lock_file().await.unwrap();

    let url_or_path = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
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
            .with_subdir(Platform::current().expect("host platform"))
            .finish(),
    );
    package_database.add_package(
        Package::build("python", "3.12.1")
            .with_subdir(Platform::current().expect("host platform"))
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two commits
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current().expect("host platform")])
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

    // Get the created lock file
    let lock = pixi.lock_file().await.unwrap();

    // previous lock file
    let previous_lock_file_str = lock.render_to_string().unwrap();

    // now run the update command to update conda packages
    // which should not trigger any update for the pinned pypi package
    pixi.update().with_package("python").await.unwrap();

    // Get the re-locked lock file
    let lock = pixi.lock_file().await.unwrap();

    let new_lock_file_str = lock.render_to_string().unwrap();

    assert_eq!(
        previous_lock_file_str, new_lock_file_str,
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
            .with_subdir(Platform::current().expect("host platform"))
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two commits
    let fixture = GitRepoFixture::new("minimal-pypi-package");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current().expect("host platform")])
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

    // Get the created lock file
    let lock = pixi.lock_file().await.unwrap();

    // find the package
    let pkg = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
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

/// Regression test for https://github.com/prefix-dev/pixi/issues/6245
///
/// When an environment is removed from the manifest, the lock-file should be
/// regenerated to drop the now non-existent environment, instead of reporting
/// that it is already up-to-date.
#[tokio::test]
async fn test_removing_environment_unsatisfies_lock_file() {
    setup_tracing();

    let mut package_database = MockRepoData::default();
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("bar", "1").finish());

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let channel = url::Url::from_file_path(channel_dir.path()).unwrap();
    let platform = Platform::current().expect("host platform");

    // Start with two environments, `a` and `b`, each backed by their own feature.
    let manifest_with_both = format!(
        r#"
    [project]
    name = "test-remove-environment"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    [feature.a.dependencies]
    foo = "*"

    [feature.b.dependencies]
    bar = "*"

    [environments]
    a = {{ features = ["a"] }}
    b = {{ features = ["b"] }}
    "#
    );

    let pixi = PixiControl::from_manifest(&manifest_with_both).unwrap();

    // Solve the initial lock-file and verify both environments are present.
    let lock_file = pixi.update_lock_file().await.unwrap();
    assert!(
        lock_file.environment("a").is_some(),
        "environment `a` should be in the lock-file"
    );
    assert!(
        lock_file.environment("b").is_some(),
        "environment `b` should be in the lock-file"
    );

    // Remove environment `b` from the manifest.
    let manifest_without_b = format!(
        r#"
    [project]
    name = "test-remove-environment"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    [feature.a.dependencies]
    foo = "*"

    [environments]
    a = {{ features = ["a"] }}
    "#
    );
    pixi.update_manifest(&manifest_without_b).unwrap();

    // Re-solving should regenerate the lock-file and drop environment `b`.
    let lock_file = pixi.update_lock_file().await.unwrap();
    assert!(
        lock_file.environment("a").is_some(),
        "environment `a` should still be in the lock-file"
    );
    assert!(
        lock_file.environment("b").is_none(),
        "environment `b` should have been removed from the lock-file"
    );
}

/// Regression test for https://github.com/prefix-dev/pixi/issues/6835
///
/// When multiple packages originate from the same Git repository and branch,
/// updating one package with `pixi update <pkg>` should also update the sibling
/// packages sharing the same repository and branch rather than reporting
/// that the lock-file was already up-to-date.
#[tokio::test]
async fn test_update_shared_git_repo_multi_package() {
    setup_tracing();

    // Create local package database with Python
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::current().expect("host platform"))
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    // Create local git fixture with two packages (pkg-a, pkg-b) and two commits (v0.1.0, v0.2.0)
    let fixture = GitRepoFixture::new("multi-package-pypi");

    // Create a new project using our package database.
    pixi.init()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![Platform::current().expect("host platform")])
        .await
        .unwrap();

    // Add a dependency on `python`
    pixi.add("python").await.unwrap();

    // Write initial dependencies pinned to first_commit into the manifest
    let manifest_txt = tokio::fs::read_to_string(pixi.manifest_path())
        .await
        .unwrap();
    let initial_manifest = format!(
        r#"{manifest_txt}
[pypi-dependencies]
pkg-a = {{ git = "{base_url}", subdirectory = "pkg-a", rev = "{first_commit}" }}
pkg-b = {{ git = "{base_url}", subdirectory = "pkg-b", rev = "{first_commit}" }}
"#,
        base_url = fixture.base_url,
        first_commit = fixture.first_commit(),
    );
    tokio::fs::write(pixi.manifest_path(), initial_manifest)
        .await
        .unwrap();

    // Solve the initial lock file and verify both packages are locked to first_commit
    let lock = pixi.update_lock_file().await.unwrap();
    let pkg_a_first = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "pkg-a",
        )
        .unwrap();
    let pkg_b_first = lock
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "pkg-b",
        )
        .unwrap();

    assert_eq!(
        pkg_a_first.as_url().unwrap().fragment().unwrap(),
        fixture.first_commit()
    );
    assert_eq!(
        pkg_b_first.as_url().unwrap().fragment().unwrap(),
        fixture.first_commit()
    );

    // Remove the rev pins from the manifest so both packages track the repository branch
    let manifest_txt = tokio::fs::read_to_string(pixi.manifest_path())
        .await
        .unwrap();
    let unpinned_manifest =
        manifest_txt.replace(&format!(", rev = \"{}\"", fixture.first_commit()), "");
    tokio::fs::write(pixi.manifest_path(), unpinned_manifest)
        .await
        .unwrap();

    // Run pixi update targeting ONLY pkg-a
    pixi.update().with_package("pkg-a").await.unwrap();

    // Verify both pkg-a and pkg-b were updated to the latest commit
    let lock_updated = pixi.lock_file().await.unwrap();
    let pkg_a_updated = lock_updated
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "pkg-a",
        )
        .unwrap();
    let pkg_b_updated = lock_updated
        .get_pypi_package_url(
            consts::DEFAULT_ENVIRONMENT_NAME,
            Platform::current().expect("host platform"),
            "pkg-b",
        )
        .unwrap();

    assert_eq!(
        pkg_a_updated.as_url().unwrap().fragment().unwrap(),
        fixture.latest_commit(),
        "targeted package pkg-a should update to the latest commit"
    );
    assert_eq!(
        pkg_b_updated.as_url().unwrap().fragment().unwrap(),
        fixture.latest_commit(),
        "sibling package pkg-b sharing the same git repo should also update to the latest commit"
    );
}
