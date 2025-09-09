use std::str::FromStr;

use pixi_cli::cli_config::GitRev;
use pixi_consts::consts;
use pixi_core::{DependencyType, Workspace};
use pixi_manifest::{FeaturesExt, SpecType};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName, VersionOrStar};
use rattler_conda_types::{PackageName, Platform};
use tempfile::TempDir;
use url::Url;

use crate::common::{
    LockFileExt, PixiControl,
    builders::{HasDependencyConfig, HasLockFileUpdateConfig, HasNoInstallConfig},
    package_database::{Package, PackageDatabase},
};
use crate::setup_tracing;

/// Test add functionality for different types of packages.
/// Run, dev, build
#[tokio::test]
async fn add_functionality() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(Package::build("rattler", "1").finish());
    package_database.add_package(Package::build("rattler", "2").finish());
    package_database.add_package(Package::build("rattler", "3").finish());

    // Write the repodata to disk
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

    // Add a package
    pixi.add("rattler==1").await.unwrap();
    pixi.add("rattler==2")
        .set_type(DependencyType::CondaDependency(SpecType::Host))
        .await
        .unwrap();
    pixi.add("rattler==3")
        .set_type(DependencyType::CondaDependency(SpecType::Build))
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "rattler==3"
    ));
    assert!(!lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "rattler==2"
    ));
    assert!(!lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "rattler==1"
    ));

    // remove the package, using matchspec
    pixi.remove("rattler==1").await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert!(!lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "rattler==1"
    ));
}

/// Test adding a package with a specific channel
#[tokio::test]
async fn add_with_channel() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    pixi.init().no_fast_prefix_overwrite(true).await.unwrap();

    pixi.add("conda-forge::py_rattler")
        .with_install(false)
        .with_frozen(true)
        .await
        .unwrap();

    pixi.add("https://prefix.dev/conda-forge::_r-mutex")
        .with_install(false)
        .with_frozen(true)
        .await
        .unwrap();

    let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();
    let mut specs = project
        .default_environment()
        .combined_dependencies(Some(Platform::current()))
        .into_specs();

    let (name, spec) = specs.next().unwrap();
    assert_eq!(name, PackageName::try_from("py_rattler").unwrap());
    assert_eq!(
        spec.into_detailed().unwrap().channel.unwrap().as_str(),
        "conda-forge"
    );

    let (name, spec) = specs.next().unwrap();
    assert_eq!(name, PackageName::try_from("_r-mutex").unwrap());
    assert_eq!(
        spec.into_detailed().unwrap().channel.unwrap().as_str(),
        "https://prefix.dev/conda-forge"
    );
}

/// Test that we get the union of all packages in the lockfile for the run,
/// build and host
#[tokio::test]
async fn add_functionality_union() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(Package::build("rattler", "1").finish());
    package_database.add_package(Package::build("libcomputer", "1.2").finish());
    package_database.add_package(Package::build("libidk", "3.1").finish());

    // Write the repodata to disk
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

    // Add a package
    pixi.add("rattler").await.unwrap();
    pixi.add("libcomputer")
        .set_type(DependencyType::CondaDependency(SpecType::Host))
        .await
        .unwrap();
    pixi.add("libidk")
        .set_type(DependencyType::CondaDependency(SpecType::Build))
        .await
        .unwrap();

    // Toml should contain the correct sections
    // We test if the toml file that is saved is correct
    // by checking if we get the correct values back in the manifest
    // We know this works because we test the manifest in another test
    // Where we check if the sections are put in the correct variables
    let project = pixi.workspace().unwrap();

    // Should contain all added dependencies
    let dependencies = project
        .default_environment()
        .dependencies(SpecType::Run, Some(Platform::current()));
    let (name, _) = dependencies.into_specs().next().unwrap();
    assert_eq!(name, PackageName::try_from("rattler").unwrap());
    let host_deps = project
        .default_environment()
        .dependencies(SpecType::Host, Some(Platform::current()));
    let (name, _) = host_deps.into_specs().next().unwrap();
    assert_eq!(name, PackageName::try_from("libcomputer").unwrap());
    let build_deps = project
        .default_environment()
        .dependencies(SpecType::Build, Some(Platform::current()));
    let (name, _) = build_deps.into_specs().next().unwrap();
    assert_eq!(name, PackageName::try_from("libidk").unwrap());

    // Lock file should contain all packages as well
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "rattler==1"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "libcomputer==1.2"
    ));
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "libidk==3.1"
    ));
}

/// Test adding a package for a specific OS
#[tokio::test]
async fn add_functionality_os() {
    setup_tracing();

    let mut package_database = PackageDatabase::default();

    // Add a package `foo` that depends on `bar` both set to version 1.
    package_database.add_package(
        Package::build("rattler", "1")
            .with_subdir(Platform::LinuxS390X)
            .finish(),
    );

    // Write the repodata to disk
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

    // Add a package
    pixi.add("rattler==1")
        .set_platforms(&[Platform::LinuxS390X])
        .set_type(DependencyType::CondaDependency(SpecType::Host))
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_match_spec(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::LinuxS390X,
        "rattler==1"
    ));
}

/// Test the `pixi add --pypi` functionality
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn add_pypi_functionality() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    pixi.init().await.unwrap();

    // Add python
    pixi.add("python~=3.12.0")
        .set_type(DependencyType::CondaDependency(SpecType::Run))
        .with_install(false)
        .await
        .unwrap();

    // Add a pypi package that is a wheel
    // without installing should succeed
    pixi.add("pipx==1.7.1")
        .set_type(DependencyType::PypiDependency)
        .with_install(false)
        .await
        .unwrap();

    // Add a pypi package to a target with short hash
    pixi.add("boltons @ git+https://github.com/mahmoud/boltons.git@d463c60")
        .set_type(DependencyType::PypiDependency)
        .with_install(true)
        .set_platforms(&[Platform::Osx64])
        .await
        .unwrap();

    // Add a pypi package to a target with extras
    pixi.add("pytest[dev]==8.3.2")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .with_install(true)
        .await
        .unwrap();

    // Read project from file and check if the dev extras are added.
    let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();
    project
        .default_environment()
        .pypi_dependencies(None)
        .into_specs()
        .for_each(|(name, spec)| {
            if name == PypiPackageName::from_str("pytest").unwrap() {
                assert_eq!(
                    spec.extras(),
                    &[pep508_rs::ExtraName::from_str("dev").unwrap()]
                );
            }
        });

    // Test all the added packages are in the lock file
    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pypi_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "pipx"
    ));
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Osx64,
        pep508_rs::Requirement::from_str("boltons").unwrap()
    ));
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        pep508_rs::Requirement::from_str("pytest").unwrap(),
    ));
    // Test that the dev extras are added, mock is a test dependency of
    // `pytest==8.3.2`
    assert!(lock.contains_pep508_requirement(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        pep508_rs::Requirement::from_str("mock").unwrap(),
    ));

    // Add a pypi package with a git url
    pixi.add("httpx @ git+https://github.com/encode/httpx.git")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .with_install(true)
        .await
        .unwrap();

    pixi.add("isort @ git+https://github.com/PyCQA/isort@c655831799765e9593989ee12faba13b6ca391a5")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .with_install(true)
        .await
        .unwrap();

    pixi.add("pytest @ https://github.com/pytest-dev/pytest/releases/download/8.2.0/pytest-8.2.0-py3-none-any.whl")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .with_install(true)
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    assert!(lock.contains_pypi_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        "httpx"
    ));
    assert!(lock.contains_pypi_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        "isort"
    ));
    assert!(lock.contains_pypi_package(
        consts::DEFAULT_ENVIRONMENT_NAME,
        Platform::Linux64,
        "pytest"
    ));
}

/// Test the `pixi add --pypi` functionality with extras
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn add_pypi_extra_functionality() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    pixi.init().await.unwrap();

    // Add python
    pixi.add("python")
        .set_type(DependencyType::CondaDependency(SpecType::Run))
        .with_install(false)
        .await
        .unwrap();

    pixi.add("black")
        .set_type(DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Add dep with extra
    pixi.add("black[cli]")
        .set_type(DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Check if the extras are added
    let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();
    project
        .default_environment()
        .pypi_dependencies(None)
        .into_specs()
        .for_each(|(name, spec)| {
            if name == PypiPackageName::from_str("black").unwrap() {
                assert_eq!(
                    spec.extras(),
                    &[pep508_rs::ExtraName::from_str("cli").unwrap()]
                );
            }
        });

    // Remove extras
    pixi.add("black")
        .set_type(DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Check if the extras are removed
    let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();
    project
        .default_environment()
        .pypi_dependencies(None)
        .into_specs()
        .for_each(|(name, spec)| {
            if name == PypiPackageName::from_str("black").unwrap() {
                assert_eq!(spec.extras(), &[]);
            }
        });

    // Add dep with extra and version
    pixi.add("black[cli]==24.8.0")
        .set_type(DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();

    // Check if the extras added and the version is set
    let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();
    project
        .default_environment()
        .pypi_dependencies(None)
        .into_specs()
        .for_each(|(name, spec)| {
            if name == PypiPackageName::from_str("black").unwrap() {
                assert_eq!(
                    spec,
                    PixiPypiSpec::Version {
                        version: VersionOrStar::from_str("==24.8.0").unwrap(),
                        extras: vec![pep508_rs::ExtraName::from_str("cli").unwrap()],
                        index: None
                    }
                );
            }
        });
}

/// Test the sdist support for pypi packages
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn add_sdist_functionality() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    pixi.init().await.unwrap();

    // Add python
    pixi.add("python")
        .set_type(DependencyType::CondaDependency(SpecType::Run))
        .with_install(true)
        .await
        .unwrap();

    // Add the sdist pypi package
    pixi.add("sdist")
        .set_type(DependencyType::PypiDependency)
        .with_install(true)
        .await
        .unwrap();
}

#[tokio::test]
async fn add_unconstrained_dependency() {
    setup_tracing();

    // Create a channel with a single package
    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("foobar", "1").finish());
    package_database.add_package(Package::build("bar", "1").finish());
    let local_channel = package_database.into_channel().await.unwrap();

    // Initialize a new pixi project using the above channel
    let pixi = PixiControl::new().unwrap();
    pixi.init().with_channel(local_channel.url()).await.unwrap();

    // Add the `packages` to the project
    pixi.add("foobar").await.unwrap();
    pixi.add("bar").with_feature("unreferenced").await.unwrap();

    let project = pixi.workspace().unwrap();

    // Get the specs for the `foobar` package
    let foo_spec = project
        .workspace
        .value
        .default_feature()
        .combined_dependencies(None)
        .unwrap_or_default()
        .get("foobar")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();

    // Get the specs for the `bar` package
    let bar_spec = project
        .workspace
        .value
        .feature("unreferenced")
        .expect("feature 'unreferenced' is missing")
        .combined_dependencies(None)
        .unwrap_or_default()
        .get("bar")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();

    insta::assert_snapshot!(format!("foobar = {foo_spec}\nbar = {bar_spec}"), @r###"
    foobar = ">=1,<2"
    bar = "*"
    "###);
}

#[tokio::test]
async fn pinning_dependency() {
    setup_tracing();

    // Create a channel with a single package
    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("foobar", "1").finish());
    package_database.add_package(Package::build("python", "3.13").finish());

    let local_channel = package_database.into_channel().await.unwrap();

    // Initialize a new pixi project using the above channel
    let pixi = PixiControl::new().unwrap();
    pixi.init().with_channel(local_channel.url()).await.unwrap();

    // Add the `packages` to the project
    pixi.add("foobar").await.unwrap();
    pixi.add("python").await.unwrap();

    let project = pixi.workspace().unwrap();

    // Get the specs for the `python` package
    let python_spec = project
        .workspace
        .value
        .default_feature()
        .dependencies(SpecType::Run, None)
        .unwrap_or_default()
        .get("python")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();
    // Testing to see if edge cases are handled correctly
    // Python shouldn't be automatically pinned to a major version.
    assert_eq!(python_spec, r#"">=3.13,<3.14""#);

    // Get the specs for the `foobar` package
    let foobar_spec = project
        .workspace
        .value
        .default_feature()
        .dependencies(SpecType::Run, None)
        .unwrap_or_default()
        .get("foobar")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();
    assert_eq!(foobar_spec, r#"">=1,<2""#);

    // Add the `python` package with a specific version
    pixi.add("python==3.13").await.unwrap();
    let project = pixi.workspace().unwrap();
    let python_spec = project
        .workspace
        .value
        .default_feature()
        .dependencies(SpecType::Run, None)
        .unwrap_or_default()
        .get("python")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();
    assert_eq!(python_spec, r#""==3.13""#);
}

#[tokio::test]
async fn add_dependency_pinning_strategy() {
    setup_tracing();

    // Create a channel with two packages
    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("python", "3.13").finish());

    let local_channel = package_database.into_channel().await.unwrap();

    // Initialize a new pixi project using the above channel
    let pixi = PixiControl::new().unwrap();
    pixi.init().with_channel(local_channel.url()).await.unwrap();

    // Add the `packages` to the project
    pixi.add_multiple(vec!["foo", "python", "bar"])
        .await
        .unwrap();

    let project = pixi.workspace().unwrap();

    // Get the specs for the `foo` package
    let foo_spec = project
        .workspace
        .value
        .default_feature()
        .dependencies(SpecType::Run, None)
        .unwrap_or_default()
        .get("foo")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();
    assert_eq!(foo_spec, r#"">=1,<2""#);

    // Get the specs for the `python` package
    let python_spec = project
        .workspace
        .value
        .default_feature()
        .dependencies(SpecType::Run, None)
        .unwrap_or_default()
        .get("python")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();
    // Testing to see if edge cases are handled correctly
    // Python shouldn't be automatically pinned to a major version.
    assert_eq!(python_spec, r#"">=3.13,<3.14""#);

    // Get the specs for the `bar` package
    let bar_spec = project
        .workspace
        .value
        .default_feature()
        .dependencies(SpecType::Run, None)
        .unwrap_or_default()
        .get("bar")
        .cloned()
        .unwrap()
        .to_toml_value()
        .to_string();
    // Testing to make sure bugfix did not regress
    // Package should be automatically pinned to a major version
    assert_eq!(bar_spec, r#"">=1,<2""#);
}

/// Test adding a git dependency with a specific branch
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn add_git_deps() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[project]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64"]
preview = ['pixi-build']
"#,
    )
    .unwrap();

    // Add a package
    pixi.add("boost-check")
        .with_git_url(Url::parse("https://github.com/wolfv/pixi-build-examples.git").unwrap())
        .with_git_rev(GitRev::new().with_branch("main".to_string()))
        .with_git_subdir("boost-check".to_string())
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    let git_package = lock
        .default_environment()
        .unwrap()
        .packages(Platform::Win64)
        .unwrap()
        .find(|p| p.as_conda().unwrap().location().as_str().contains("git+"));

    insta::with_settings!({filters => vec![
        (r"#([a-f0-9]+)", "#[FULL_COMMIT]"),
    ]}, {
        insta::assert_snapshot!(git_package.unwrap().as_conda().unwrap().location());

    });

    // Check the manifest itself
    insta::assert_snapshot!(
        pixi.workspace()
            .unwrap()
            .workspace
            .provenance
            .read()
            .unwrap()
            .into_inner()
    );
}

/// Test adding git dependencies with credentials
/// This tests is skipped on windows because it spawns a credential helper
/// during the CI run
#[cfg(not(windows))]
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn add_git_deps_with_creds() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[project]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64"]
preview = ['pixi-build']
"#,
    )
    .unwrap();

    // Add a package
    // we want to make sure that the credentials are not exposed in the lock file
    pixi.add("boost-check")
        .with_git_url(
            Url::parse("https://user:token123@github.com/wolfv/pixi-build-examples.git").unwrap(),
        )
        .with_git_rev(GitRev::new().with_branch("main".to_string()))
        .with_git_subdir("boost-check".to_string())
        .await
        .unwrap();

    let lock = pixi.lock_file().await.unwrap();
    let git_package = lock
        .default_environment()
        .unwrap()
        .packages(Platform::Linux64)
        .unwrap()
        .find(|p| p.as_conda().unwrap().location().as_str().contains("git+"));

    insta::with_settings!({filters => vec![
        (r"#([a-f0-9]+)", "#[FULL_COMMIT]"),
    ]}, {
        insta::assert_snapshot!(git_package.unwrap().as_conda().unwrap().location());

    });

    // Check the manifest itself
    insta::assert_snapshot!(
        pixi.workspace()
            .unwrap()
            .modify()
            .unwrap()
            .manifest()
            .document
            .to_string()
    );
}

/// Test adding a git dependency with a specific commit
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn add_git_with_specific_commit() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[project]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64"]
preview = ['pixi-build']"#,
    )
    .unwrap();

    // Add a package
    pixi.add("boost-check")
        .with_git_url(Url::parse("https://github.com/wolfv/pixi-build-examples.git").unwrap())
        .with_git_rev(GitRev::new().with_rev("8a1d9b9".to_string()))
        .with_git_subdir("boost-check".to_string())
        .await
        .unwrap();

    // Check the lock file
    let lock = pixi.lock_file().await.unwrap();
    let git_package = lock
        .default_environment()
        .unwrap()
        .packages(Platform::Win64)
        .unwrap()
        .find(|p| p.as_conda().unwrap().location().as_str().contains("git+"));

    insta::with_settings!({filters => vec![
        (r"#([a-f0-9]+)", "#[FULL_COMMIT]"),
    ]}, {
        insta::assert_snapshot!(git_package.unwrap().as_conda().unwrap().location());

    });

    // Check the manifest itself
    insta::assert_snapshot!(
        pixi.workspace()
            .unwrap()
            .workspace
            .provenance
            .read()
            .unwrap()
            .into_inner()
    );
}

/// Test adding a git dependency with a specific tag
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn add_git_with_tag() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[project]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64"]
preview = ['pixi-build']"#,
    )
    .unwrap();

    // Add a package
    pixi.add("boost-check")
        .with_git_url(Url::parse("https://github.com/wolfv/pixi-build-examples.git").unwrap())
        .with_git_rev(
            GitRev::new().with_rev("8a1d9b9b1755825165a615d563966aaa59a5361c".to_string()),
        )
        .with_git_subdir("boost-check".to_string())
        .await
        .unwrap();

    // Check the lock file
    let lock = pixi.lock_file().await.unwrap();
    let git_package = lock
        .default_environment()
        .unwrap()
        .packages(Platform::Win64)
        .unwrap()
        .find(|p| p.as_conda().unwrap().location().as_str().contains("git+"));

    insta::with_settings!({filters => vec![
        (r"#([a-f0-9]+)", "#[FULL_COMMIT]"),
        (r"rev=([a-f0-9]+)", "rev=[REV]"),
    ]}, {
        insta::assert_snapshot!(git_package.unwrap().as_conda().unwrap().location());
    });

    // Check the manifest itself
    insta::assert_snapshot!(
        pixi.workspace()
            .unwrap()
            .workspace
            .provenance
            .read()
            .unwrap()
            .into_inner()
    );
}

/// Test adding a git dependency using ssh url
#[tokio::test]
async fn add_plain_ssh_url() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[project]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64"]
preview = ['pixi-build']"#,
    )
    .unwrap();

    // Add a package
    pixi.add("boost-check")
        .with_git_url(Url::parse("git+ssh://git@github.com/wolfv/pixi-build-examples.git").unwrap())
        .with_install(false)
        .with_frozen(true)
        .await
        .unwrap();

    // Check the manifest itself
    insta::assert_snapshot!(
        pixi.workspace()
            .unwrap()
            .workspace
            .provenance
            .read()
            .unwrap()
            .into_inner()
    );
}

/// Test adding a git dependency using ssh url
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn add_pypi_git() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        format!(
            r#"
[project]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{platform}"]

"#,
            platform = Platform::current()
        )
        .as_str(),
    )
    .unwrap();

    // Add python
    pixi.add("python>=3.13.2,<3.14").await.unwrap();

    // Add a package
    pixi.add("boltons")
        .set_pypi(true)
        .with_git_url(Url::parse("https://github.com/mahmoud/boltons.git").unwrap())
        .await
        .unwrap();

    // Check the manifest itself
    insta::with_settings!({filters => vec![
        (r"#([a-f0-9]+)", "#[FULL_COMMIT]"),
        (r"platforms = \[.*\]", "platforms = [\"<PLATFORM>\"]"),
    ]}, {
        insta::assert_snapshot!(pixi.workspace().unwrap().workspace.provenance.read().unwrap().into_inner());
    });

    let lock_file = pixi.lock_file().await.unwrap();

    let (boltons, _) = lock_file
        .default_environment()
        .unwrap()
        .pypi_packages(Platform::current())
        .unwrap()
        .find(|(p, _)| p.name.to_string() == "boltons")
        .unwrap();

    insta::with_settings!( {filters => vec![
        (r"#([a-f0-9]+)", "#[FULL_COMMIT]"),
    ]}, {
        insta::assert_snapshot!(boltons.location);
    });
}

#[tokio::test]
async fn add_git_dependency_without_preview_feature_fails() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[workspace]
name = "test-git-no-preview"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64"]
"#,
    )
    .unwrap();

    let result = pixi
        .add("boost-check")
        .with_git_url(Url::parse("https://github.com/wolfv/pixi-build-examples.git").unwrap())
        .with_git_subdir("boost-check".to_string())
        .await;

    assert!(result.is_err());
    let error = result.unwrap_err();

    // Use insta to snapshot test the full error message format including help text
    insta::with_settings!({
        filters => vec![
            // Filter out the dynamic manifest path to make the snapshot stable
            (r"manifest \([^)]+\)", "manifest (<MANIFEST_PATH>)"),
        ]
    }, {
        insta::assert_debug_snapshot!("git_dependency_without_preview_error", error);
    });
}

#[tokio::test]
async fn add_git_dependency_with_preview_feature_succeeds() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[workspace]
name = "test-git-with-preview"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64"]
preview = ["pixi-build"]
"#,
    )
    .unwrap();

    let result = pixi
        .add("boost-check")
        .with_git_url(Url::parse("https://github.com/wolfv/pixi-build-examples.git").unwrap())
        .with_git_subdir("boost-check".to_string())
        .with_install(false)
        .with_frozen(true)
        .await;

    assert!(result.is_ok());

    let workspace = pixi.workspace().unwrap();
    let deps = workspace
        .default_environment()
        .combined_dependencies(Some(Platform::Linux64));

    let (name, spec) = deps
        .into_specs()
        .find(|(name, _)| name.as_normalized() == "boost-check")
        .unwrap();
    assert_eq!(name.as_normalized(), "boost-check");
    assert!(spec.is_source());
}

#[tokio::test]
async fn add_dependency_dont_create_project() {
    setup_tracing();

    // Create a channel with two packages
    let mut package_database = PackageDatabase::default();
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("bar", "1").finish());
    package_database.add_package(Package::build("python", "3.13").finish());

    let local_channel = package_database.into_channel().await.unwrap();

    let local_channel_str = format!("{}", local_channel.url());

    // Initialize a new pixi project using the above channel
    let pixi = PixiControl::from_manifest(&format!(
        r#"
[workspace]
name = "some-workspace"
platforms = []
channels = ['{local_channel}']
preview = ['pixi-build']
"#,
        local_channel = local_channel_str
    ))
    .unwrap();

    // Add the `packages` to the project
    pixi.add("foo").await.unwrap();

    let workspace = pixi.workspace().unwrap();

    // filter out local channels from the insta
    insta::with_settings!({filters => vec![
        (local_channel_str.as_str(), "file://<LOCAL_CHANNEL>/"),
    ]}, {
        insta::assert_snapshot!(workspace.workspace.provenance.read().unwrap().into_inner());
    });
}
