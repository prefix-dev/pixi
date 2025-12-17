use std::str::FromStr;

use pixi_cli::cli_config::GitRev;
use pixi_consts::consts;
use pixi_core::{DependencyType, Workspace};
use pixi_manifest::{FeaturesExt, SpecType};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName, VersionOrStar};
use rattler_conda_types::{PackageName, Platform};
use tempfile::TempDir;
use url::Url;

use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::BackendOverride;

use crate::common::{
    LockFileExt, PixiControl,
    builders::{HasDependencyConfig, HasLockFileUpdateConfig, HasNoInstallConfig},
};
use crate::setup_tracing;
use pixi_test_utils::{GitRepoFixture, MockRepoData, Package};

/// Test add functionality for different types of packages.
/// Run, dev, build
#[tokio::test]
async fn add_functionality() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

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
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn add_with_channel() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    pixi.init().await.unwrap();

    pixi.add("https://prefix.dev/conda-forge::_openmp_mutex")
        .with_install(false)
        .with_frozen(true)
        .await
        .unwrap();

    pixi.project_channel_add()
        .with_channel("https://prefix.dev/robostack-kilted")
        .await
        .unwrap();
    pixi.add("https://prefix.dev/robostack-kilted::ros2-distro-mutex")
        .with_install(false)
        .await
        .unwrap();

    let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();
    let mut specs = project
        .default_environment()
        .combined_dependencies(Some(Platform::current()))
        .into_specs();

    let (name, spec) = specs.next().unwrap();
    assert_eq!(name, PackageName::try_from("_openmp_mutex").unwrap());
    assert_eq!(
        spec.into_detailed().unwrap().channel.unwrap().as_str(),
        "https://prefix.dev/conda-forge"
    );

    let (name, spec) = specs.next().unwrap();
    assert_eq!(name, PackageName::try_from("ros2-distro-mutex").unwrap());
    assert_eq!(
        spec.into_detailed().unwrap().channel.unwrap().as_str(),
        "https://prefix.dev/robostack-kilted"
    );
}

/// Test that we get the union of all packages in the lockfile for the run,
/// build and host
#[tokio::test]
async fn add_functionality_union() {
    setup_tracing();

    let mut package_database = MockRepoData::default();

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

    let mut package_database = MockRepoData::default();

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

/// Test the `pixi add --pypi` functionality (using local mocks)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn add_pypi_functionality() {
    use crate::common::pypi_index::{Database as PyPIDatabase, PyPIPackage};

    setup_tracing();

    // Create local git fixtures for pypi git packages
    let boltons_fixture = GitRepoFixture::new("pypi-boltons");
    let httpx_fixture = GitRepoFixture::new("pypi-httpx");
    let isort_fixture = GitRepoFixture::new("pypi-isort");

    // Create local PyPI index with test packages
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("pipx", "1.7.1"))
        .with(
            PyPIPackage::new("pytest", "8.3.2").with_requires_dist(["mock; extra == \"dev\""]), // dev extra requires mock
        )
        .with(PyPIPackage::new("mock", "5.0.0"))
        .into_simple_index()
        .unwrap();

    // Create a separate flat index for direct wheel URL testing
    let pytest_wheel = PyPIDatabase::new()
        .with(PyPIPackage::new("pytest", "8.2.0"))
        .into_flat_index()
        .unwrap();
    let pytest_wheel_url = pytest_wheel
        .url()
        .join("pytest-8.2.0-py3-none-any.whl")
        .unwrap();

    // Create local conda channel with Python for multiple platforms
    let mut package_db = MockRepoData::default();
    for platform in [Platform::current(), Platform::Linux64, Platform::Osx64] {
        package_db.add_package(
            Package::build("python", "3.12.0")
                .with_subdir(platform)
                .finish(),
        );
    }
    let channel = package_db.into_channel().await.unwrap();

    let pixi = PixiControl::new().unwrap();

    pixi.init()
        .without_channels()
        .with_local_channel(channel.url().to_file_path().unwrap())
        .with_platforms(vec![
            Platform::current(),
            Platform::Linux64,
            Platform::Osx64,
        ])
        .await
        .unwrap();

    // Add pypi-options to the manifest
    let manifest = pixi.manifest_contents().unwrap();
    let updated_manifest = format!(
        "{}\n[pypi-options]\nindex-url = \"{}\"\n",
        manifest,
        pypi_index.index_url()
    );
    pixi.update_manifest(&updated_manifest).unwrap();

    // Add python
    pixi.add("python~=3.12.0")
        .set_type(DependencyType::CondaDependency(SpecType::Run))
        .await
        .unwrap();

    // Add a pypi package that is a wheel
    // without installing should succeed
    pixi.add("pipx==1.7.1")
        .set_type(DependencyType::PypiDependency)
        .await
        .unwrap();

    // Add a pypi package to a target with short hash (using local git fixture)
    let boltons_short_commit = &boltons_fixture.first_commit()[..7];
    pixi.add(&format!(
        "boltons @ git+{}@{}",
        boltons_fixture.base_url, boltons_short_commit
    ))
    .set_type(DependencyType::PypiDependency)
    .set_platforms(&[Platform::Osx64])
    .await
    .unwrap();

    // Add a pypi package to a target with extras
    pixi.add("pytest[dev]==8.3.2")
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
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

    // Add a pypi package with a git url (using local fixture)
    pixi.add(&format!("httpx @ git+{}", httpx_fixture.base_url))
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
        .await
        .unwrap();

    // Add with specific commit (using local fixture)
    let isort_commit = isort_fixture.first_commit();
    pixi.add(&format!(
        "isort @ git+{}@{}",
        isort_fixture.base_url, isort_commit
    ))
    .set_type(DependencyType::PypiDependency)
    .set_platforms(&[Platform::Linux64])
    .await
    .unwrap();

    // Add pytest from direct wheel URL (using local wheel file)
    pixi.add(&format!("pytest @ {pytest_wheel_url}"))
        .set_type(DependencyType::PypiDependency)
        .set_platforms(&[Platform::Linux64])
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

/// Test the `pixi add --pypi` functionality with extras (using local mocks)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn add_pypi_extra_functionality() {
    use crate::common::pypi_index::{Database as PyPIDatabase, PyPIPackage};

    setup_tracing();

    // Create local PyPI index with black package (multiple versions, with cli extra)
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("black", "24.8.0"))
        .with(PyPIPackage::new("black", "24.7.0"))
        .with(PyPIPackage::new("click", "8.0.0")) // cli extra dependency
        .into_simple_index()
        .unwrap();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    let channel_url = channel.url();
    let index_url = pypi_index.index_url();
    let platform = Platform::current();

    // Create manifest with local channel and pypi index
    let pixi = PixiControl::from_manifest(&format!(
        r#"
[workspace]
name = "test-pypi-extras"
channels = ["{channel_url}"]
platforms = ["{platform}"]
conda-pypi-map = {{}} # disable mapping

[dependencies]
python = "==3.12.0"

[pypi-options]
index-url = "{index_url}"
"#
    ))
    .unwrap();

    pixi.add("black")
        .set_type(DependencyType::PypiDependency)
        .await
        .unwrap();

    // Add dep with extra
    pixi.add("black[cli]")
        .set_type(DependencyType::PypiDependency)
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
#[cfg_attr(
    any(not(feature = "slow_integration_tests"), not(feature = "online_tests")),
    ignore
)]
async fn add_sdist_functionality() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();

    pixi.init().await.unwrap();

    // Add python
    pixi.add("python")
        .set_type(DependencyType::CondaDependency(SpecType::Run))
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
    let mut package_database = MockRepoData::default();
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
        .get_single("foobar")
        .unwrap()
        .unwrap()
        .clone()
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
        .get_single("bar")
        .unwrap()
        .unwrap()
        .clone()
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
    let mut package_database = MockRepoData::default();
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
        .get_single("python")
        .unwrap()
        .unwrap()
        .clone()
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
        .get_single("foobar")
        .unwrap()
        .unwrap()
        .clone()
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
        .get_single("python")
        .unwrap()
        .unwrap()
        .clone()
        .to_toml_value()
        .to_string();
    assert_eq!(python_spec, r#""==3.13""#);
}

#[tokio::test]
async fn add_dependency_pinning_strategy() {
    setup_tracing();

    // Create a channel with two packages
    let mut package_database = MockRepoData::default();
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
        .get_single("foo")
        .unwrap()
        .unwrap()
        .clone()
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
        .get_single("python")
        .unwrap()
        .unwrap()
        .clone()
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
        .get_single("bar")
        .unwrap()
        .unwrap()
        .clone()
        .to_toml_value()
        .to_string();
    // Testing to make sure bugfix did not regressed
    // Package should be automatically pinned to a major version
    assert_eq!(bar_spec, r#"">=1,<2""#);
}

/// Test adding a git dependency with a specific branch (using local fixture)
#[tokio::test]
async fn add_git_deps() {
    setup_tracing();

    // Create local git fixture with passthrough backend
    let fixture = GitRepoFixture::new("conda-build-package");
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());

    let pixi = PixiControl::from_manifest(
        r#"
[workspace]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64"]
preview = ['pixi-build']
"#,
    )
    .unwrap()
    .with_backend_override(backend_override);

    // Add a package using local git fixture URL
    pixi.add("boost-check")
        .with_git_url(fixture.base_url.clone())
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

    let location = git_package
        .unwrap()
        .as_conda()
        .unwrap()
        .location()
        .to_string();

    insta::with_settings!({filters => vec![
        (r"file://[^?#]+", "file://[TEMP_PATH]"),
        (r"#[a-f0-9]+", "#[COMMIT]"),
    ]}, {
        insta::assert_snapshot!(location, @"git+file://[TEMP_PATH]?subdirectory=boost-check&branch=main#[COMMIT]");
    });
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
[workspace]
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

/// Test adding a git dependency with a specific commit (using local fixture)
#[tokio::test]
async fn add_git_with_specific_commit() {
    setup_tracing();

    // Create local git fixture with passthrough backend
    let fixture = GitRepoFixture::new("conda-build-package");
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());

    let pixi = PixiControl::from_manifest(
        r#"
[workspace]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64"]
preview = ['pixi-build']"#,
    )
    .unwrap()
    .with_backend_override(backend_override);

    // Add a package using the first commit from our fixture
    let first_commit = fixture.first_commit().to_string();
    let short_commit = &first_commit[..7]; // Use short hash like the original test

    pixi.add("boost-check")
        .with_git_url(fixture.base_url.clone())
        .with_git_rev(GitRev::new().with_rev(short_commit.to_string()))
        .with_git_subdir("boost-check".to_string())
        .await
        .unwrap();

    // Check the lock file
    let lock = pixi.lock_file().await.unwrap();
    let git_package = lock
        .default_environment()
        .unwrap()
        .packages(Platform::Linux64)
        .unwrap()
        .find(|p| p.as_conda().unwrap().location().as_str().contains("git+"));

    let location = git_package
        .unwrap()
        .as_conda()
        .unwrap()
        .location()
        .to_string();

    insta::with_settings!({filters => vec![
        (r"file://[^?#]+", "file://[TEMP_PATH]"),
        (r"rev=[a-f0-9]+", "rev=[SHORT_COMMIT]"),
        (r"#[a-f0-9]+", "#[FULL_COMMIT]"),
    ]}, {
        insta::assert_snapshot!(location, @"git+file://[TEMP_PATH]?subdirectory=boost-check&rev=[SHORT_COMMIT]#[FULL_COMMIT]");
    });
}

/// Test adding a git dependency with a specific tag (using local fixture)
#[tokio::test]
async fn add_git_with_tag() {
    setup_tracing();

    // Create local git fixture with passthrough backend
    // The fixture creates a tag "v0.1.0" for the second commit
    let fixture = GitRepoFixture::new("conda-build-package");
    let backend_override = BackendOverride::from_memory(PassthroughBackend::instantiator());

    let pixi = PixiControl::from_manifest(
        r#"
[workspace]
name = "test-channel-change"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64"]
preview = ['pixi-build']"#,
    )
    .unwrap()
    .with_backend_override(backend_override);

    // Add a package using the tag from our fixture
    let tag_commit = fixture.tag_commit("v0.1.0").to_string();

    pixi.add("boost-check")
        .with_git_url(fixture.base_url.clone())
        .with_git_rev(GitRev::new().with_tag("v0.1.0".to_string()))
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

    let location = git_package
        .unwrap()
        .as_conda()
        .unwrap()
        .location()
        .to_string();

    insta::with_settings!({filters => vec![
        (r"file://[^?#]+", "file://[TEMP_PATH]"),
        (r"#[a-f0-9]+", "#[COMMIT]"),
    ]}, {
        insta::assert_snapshot!(location, @"git+file://[TEMP_PATH]?subdirectory=boost-check&tag=v0.1.0#[COMMIT]");
    });

    // Verify the commit hash matches the tag's commit
    assert!(
        location.ends_with(&format!("#{tag_commit}")),
        "Expected tag to resolve to commit {tag_commit}, got {location}"
    );
}

/// Test adding a git dependency using ssh url
#[tokio::test]
async fn add_plain_ssh_url() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
[workspace]
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
[workspace]
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
    let mut package_database = MockRepoData::default();
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
channels = ['{local_channel_str}']
preview = ['pixi-build']
"#
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
    /// Test the `pixi add --pypi --index` functionality
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]

    async fn add_pypi_with_index() {
        use crate::common::pypi_index::{Database as PyPIDatabase, PyPIPackage};
        setup_tracing();

        // Create local PyPI index with test package

        let pypi_index = PyPIDatabase::new()
            .with(PyPIPackage::new("requests", "2.32.0"))
            .into_simple_index()
            .unwrap();

        // Create local conda channel with Python

        let mut package_db = MockRepoData::default();

        package_db.add_package(
            Package::build("python", "3.12.0")
                .with_subdir(Platform::current())
                .finish(),
        );

        let channel = package_db.into_channel().await.unwrap();

        let pixi = PixiControl::new().unwrap();

        pixi.init()
            .without_channels()
            .with_local_channel(channel.url().to_file_path().unwrap())
            .await
            .unwrap();

        // Add python

        pixi.add("python==3.12.0")
            .set_type(DependencyType::CondaDependency(SpecType::Run))
            .await
            .unwrap();

        // Add a pypi package with custom index

        let custom_index = pypi_index.index_url().to_string();

        pixi.add("requests")
            .set_type(DependencyType::PypiDependency)
            .with_index(custom_index.clone())
            .await
            .unwrap();

        // Read project and check if index is set

        let project = Workspace::from_path(pixi.manifest_path().as_path()).unwrap();

        let pypi_deps: Vec<_> = project
            .default_environment()
            .pypi_dependencies(None)
            .into_specs()
            .collect();

        // Find the requests package

        let (_name, spec) = pypi_deps
            .iter()
            .find(|(name, _)| *name == PypiPackageName::from_str("requests").unwrap())
            .expect("requests package should be in dependencies");

        // Verify the index is set correctly

        if let PixiPypiSpec::Version { index, .. } = spec {
            assert_eq!(
                index.as_ref().map(|u| u.as_str()),
                Some(custom_index.as_str()),
                "Index URL should match the provided custom index"
            );
        } else {
            panic!("Expected PixiPypiSpec::Version variant");
        }
    }
}
