use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    time::SystemTime,
};

use pep508_rs::Requirement;
use rattler_conda_types::Platform;
use tempfile::tempdir;
use typed_path::Utf8TypedPath;

use crate::common::pypi_index::{Database as PyPIDatabase, PyPIPackage};
use crate::common::{LockFileExt, PixiControl};
use crate::setup_tracing;
use pixi_test_utils::{MockRepoData, Package};

/// This tests if we can resolve pyproject optional dependencies recursively
/// before when running `pixi list -e all`, this would have not included numpy
/// we are now explicitly testing that this works
#[tokio::test]
async fn pyproject_optional_dependencies_resolve_recursively() {
    setup_tracing();

    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("numpy", "1.0.0"))
        .with(PyPIPackage::new("sphinx", "1.0.0"))
        .with(PyPIPackage::new("pytest", "1.0.0"))
        .into_simple_index()
        .unwrap();

    let platform = Platform::current();
    let platform_str = platform.to_string();

    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();
    let channel_url = channel.url();
    let index_url = simple.index_url();

    let pyproject = format!(
        r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "recursive-optional-groups"

[project.optional-dependencies]
np = ["numpy"]
all = ["recursive-optional-groups[np]"]

[dependency-groups]
docs = ["sphinx"]
test = ["recursive-optional-groups[np]", "pytest", {{include-group = "docs"}}]

[tool.pixi.workspace]
channels = ["{channel_url}"]
platforms = ["{platform_str}"]
conda-pypi-map = {{}}

[tool.pixi.dependencies]
python = "==3.11.0"

[tool.pixi.pypi-options]
index-url = "{index_url}"

[tool.pixi.environments]
np = {{features = ["np"]}}
all = {{features = ["all"]}}
test = {{features = ["test"]}}
"#,
    );

    let pixi = PixiControl::from_pyproject_manifest(&pyproject).unwrap();

    let lock = pixi.update_lock_file().await.unwrap();

    let numpy_req = Requirement::from_str("numpy").unwrap();
    let sphinx_req = Requirement::from_str("sphinx").unwrap();
    assert!(
        lock.contains_pep508_requirement("np", platform, numpy_req.clone()),
        "np environment should include numpy from optional dependencies"
    );
    assert!(
        lock.contains_pep508_requirement("all", platform, numpy_req.clone()),
        "all environment should include numpy inherited from recursive optional dependency"
    );
    assert!(
        lock.contains_pep508_requirement("test", platform, numpy_req),
        "test environment should include numpy inherited from recursive optional dependency"
    );
    assert!(
        lock.contains_pep508_requirement("test", platform, sphinx_req),
        "test environment should include sphinx inherited from recursive dependency group"
    );
}

#[tokio::test]
async fn test_flat_links_based_index_returns_path() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Build a local flat (find-links) index with a single wheel: foo==1.0.0
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_flat_index()
        .expect("failed to create local flat index");

    let find_links_path = index.path().display().to_string().replace('\\', "/");

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-flat-find-links"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        find-links = [{{ path = "{find_links_path}"}}]
        "#,
        platform = platform,
        channel_url = channel.url(),
        find_links_path = find_links_path,
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    // Expect the locked URL to be a local path pointing at our generated wheel.
    // Our wheel builder uses the tag py3-none-any by default.
    assert_eq!(
        lock_file
            .get_pypi_package_url("default", platform, "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*index.path().as_os_str().to_string_lossy())
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

#[tokio::test]
async fn test_file_based_index_returns_path() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .expect("failed to create simple index");

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        extra-index-urls = [
            "{index_url}"
        ]"#,
        platform = platform,
        channel_url = channel.url(),
        index_url = simple.index_url(),
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", platform, "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*simple.index_path().as_os_str().to_string_lossy())
            .join("foo")
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_index_strategy() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    let idx_a = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .unwrap();
    let idx_b = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "2.0.0"))
        .into_simple_index()
        .unwrap();
    let idx_c = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "3.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        extra-index-urls = [
            "{idx_a}",
            "{idx_b}",
            "{idx_c}",
        ]

        [feature.first-index.pypi-options]
        index-strategy = "first-index"

        [feature.unsafe-first-match-unconstrained.pypi-options]
        index-strategy = "unsafe-first-match"

        [feature.unsafe-first-match-constrained.pypi-options]
        index-strategy = "unsafe-first-match"

        [feature.unsafe-first-match-constrained.pypi-dependencies]
        foo = "==3.0.0"

        [feature.unsafe-best-match.pypi-options]
        index-strategy = "unsafe-best-match"

        [environments]
        default = ["first-index"]
        unsafe-first-match-unconstrained = ["unsafe-first-match-unconstrained"]
        unsafe-first-match-constrained = ["unsafe-first-match-constrained"]
        unsafe-best-match = ["unsafe-best-match"]
        "#,
        platform = platform,
        channel_url = channel.url(),
        idx_a = idx_a.index_url(),
        idx_b = idx_b.index_url(),
        idx_c = idx_c.index_url(),
    ));

    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file.get_pypi_package_version("default", platform, "foo"),
        Some("1.0.0".into())
    );
    assert_eq!(
        lock_file.get_pypi_package_version("unsafe-first-match-unconstrained", platform, "foo"),
        Some("1.0.0".into())
    );

    assert_eq!(
        lock_file.get_pypi_package_version("unsafe-first-match-constrained", platform, "foo"),
        Some("3.0.0".into())
    );
    assert_eq!(
        lock_file.get_pypi_package_version("unsafe-best-match", platform, "foo"),
        Some("3.0.0".into())
    );
}

#[tokio::test]
/// This test checks if we can pin a package from a PyPI index, by explicitly specifying the index.
async fn test_pinning_index() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    let idx = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-pinning-index"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        foo = {{ version = "*", index = "{idx_url}" }}

        "#,
        platform = platform,
        channel_url = channel.url(),
        idx_url = idx.index_url(),
    ));

    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", platform, "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*idx.index_path().as_os_str().to_string_lossy())
            .join("foo")
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
/// This test checks if we can receive torch correctly from the whl/cu124 index.
async fn pin_torch() {
    setup_tracing();

    // Do some platform magic, as the index does not contain wheels for each platform.
    let platform = Platform::current();
    let platforms = match platform {
        Platform::Linux64 => "\"linux-64\"".to_string(),
        _ => format!("\"{platform}\", \"linux-64\""),
    };

    // Create local conda channel with Python for all relevant platforms
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::Linux64)
            .finish(),
    );
    if platform != Platform::Linux64 {
        package_db.add_package(
            Package::build("python", "3.12.0")
                .with_subdir(platform)
                .finish(),
        );
    }
    let channel = package_db.into_channel().await.unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-pinning-index"
        platforms = [{platforms}]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [target.linux-64.pypi-dependencies]
        torch = {{ version = "*", index = "https://download.pytorch.org/whl/cu124" }}
        "#,
        channel_url = channel.url(),
    ));

    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();
    // So the check is as follows:
    // 1. The PyPI index is the main index-url, so normally torch would be taken from there.
    // 2. We manually check if it is taken from the whl/cu124 index instead.
    assert!(
        lock_file
            .get_pypi_package_url("default", Platform::Linux64, "torch")
            .unwrap()
            .as_url()
            .unwrap()
            .path()
            .contains("/whl/cu124")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_allow_insecure_host() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Create local PyPI index with sh package
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("sh", "2.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        sh = "*"

        [pypi-options]
        index-url = "{pypi_index_url}"
        extra-index-urls = ["https://expired.badssl.com/"]"#,
        platform = platform,
        channel_url = channel.url(),
        pypi_index_url = pypi_index.index_url(),
    ))
    .unwrap();
    // will occur ssl error
    assert!(
        pixi.update_lock_file().await.is_err(),
        "should occur ssl error"
    );

    let config_path = pixi.workspace().unwrap().pixi_dir().join("config.toml");
    fs_err::create_dir_all(config_path.parent().unwrap()).unwrap();
    let mut file = File::create(config_path).unwrap();
    file.write_all(
        r#"
        detached-environments = false

        [pypi-config]
        allow-insecure-host = ["expired.badssl.com"]"#
            .as_bytes(),
    )
    .unwrap();
    pixi.update_lock_file().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_tls_no_verify_with_pypi_dependencies() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Create local PyPI index with sh package
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("sh", "2.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-tls-test"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        sh = "*"

        [pypi-options]
        index-url = "{pypi_index_url}"
        extra-index-urls = ["https://expired.badssl.com/"]"#,
        platform = platform,
        channel_url = channel.url(),
        pypi_index_url = pypi_index.index_url(),
    ))
    .unwrap();

    // First verify that it fails with SSL errors when tls-no-verify is not set
    assert!(
        pixi.update_lock_file().await.is_err(),
        "should fail with SSL error when tls-no-verify is not enabled"
    );

    // Now set tls-no-verify = true in the project config
    let config_path = pixi.workspace().unwrap().pixi_dir().join("config.toml");
    fs_err::create_dir_all(config_path.parent().unwrap()).unwrap();
    let mut file = File::create(config_path).unwrap();
    file.write_all(
        r#"
        tls-no-verify = true"#
            .as_bytes(),
    )
    .unwrap();

    // With tls-no-verify = true, this should now succeed or fail for non-SSL reasons
    let result = pixi.update_lock_file().await;

    // The test should succeed because tls-no-verify bypasses SSL verification
    // If it fails, it should not be due to SSL certificate issues
    match result {
        Ok(_) => {
            // Success - TLS verification was bypassed
        }
        Err(e) => {
            let error_msg = format!("{e:?}");
            // If it fails, it should NOT be due to SSL/TLS certificate issues
            assert!(
                !error_msg.to_lowercase().contains("certificate")
                    && !error_msg.to_lowercase().contains("ssl")
                    && !error_msg.to_lowercase().contains("tls"),
                "Error should not be SSL/TLS related when tls-no-verify is enabled. Got: {error_msg}"
            );
        }
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_tls_verify_still_fails_without_config() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Create local PyPI index with sh package
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("sh", "2.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-tls-verify-test"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        sh = "*"

        [pypi-options]
        index-url = "{pypi_index_url}"
        extra-index-urls = ["https://expired.badssl.com/"]"#,
        platform = platform,
        channel_url = channel.url(),
        pypi_index_url = pypi_index.index_url(),
    ))
    .unwrap();

    // Without tls-no-verify, this should fail with SSL errors
    let result = pixi.update_lock_file().await;
    assert!(
        result.is_err(),
        "should fail with SSL error when tls-no-verify is not enabled"
    );

    let error = result.unwrap_err();
    let error_msg = format!("{error:?}");
    // The error should be SSL/TLS related
    assert!(
        error_msg.to_lowercase().contains("certificate")
            || error_msg.to_lowercase().contains("ssl")
            || error_msg.to_lowercase().contains("tls")
            || error_msg.contains("expired.badssl.com"),
        "Error should be SSL/TLS related. Got: {error_msg}"
    );
}

#[tokio::test]
#[cfg_attr(
    any(not(feature = "online_tests"), not(feature = "slow_integration_tests")),
    ignore
)]
async fn test_indexes_are_passed_when_solving_build_pypi_dependencies() {
    setup_tracing();

    // Provide a local simple index containing `foo` used in build-system requires.
    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .expect("failed to create simple index");

    let pixi = PixiControl::from_pyproject_manifest(&format!(
        r#"
        [project]
        name = "pypi-build-index"
        requires-python = ">=3.10"
        version = "0.1.0"

        [build-system]
        requires = [
        "foo",
        "hatchling",
        ]
        build-backend = "hatchling.build"

        [tool.hatch.build]
        include = ["src"]
        targets.wheel.strict-naming = false
        targets.wheel.packages = ["src/pypi_build_index"]
        targets.sdist.strict-naming = false
        targets.sdist.packages = ["src/pypi_build_index"]



        [tool.pixi.workspace]
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["{platform}"]

        [tool.pixi.dependencies]
        hatchling = "*"

        [tool.pixi.pypi-options]
        index-url = "{index_url}"
        no-build-isolation = ["pypi-build-index"]

        [tool.pixi.pypi-dependencies]
        pypi-build-index = {{ path = ".", editable = true }}
        "#,
        platform = Platform::current(),
        index_url = simple.index_url(),
    ))
    .unwrap();

    let project_path = pixi.workspace_path();
    let src_dir = project_path.join("src").join("pypi_build_index");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "").unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    // verify that the pypi-build-index can be installed when solved the build dependencies

    let tmp_dir = tempdir().unwrap();
    let tmp_dir_path = tmp_dir.path();

    temp_env::async_with_vars(
        [("PIXI_CACHE_DIR", Some(tmp_dir_path.to_str().unwrap()))],
        async {
            pixi.install().await.unwrap();
        },
    )
    .await;

    let mut local_pypi_index = simple.index_path().display().to_string();

    let mut lock_file_index = lock_file
        .default_environment()
        .unwrap()
        .pypi_indexes()
        .unwrap()
        .indexes
        .first()
        .unwrap()
        .path()
        .to_string();

    if cfg!(windows) {
        // Replace backslashes with forward slashes for consistency in snapshots as well
        // as ; with :
        local_pypi_index = local_pypi_index.replace("\\\\", "\\");
        local_pypi_index = local_pypi_index.replace("\\", "/");

        // pop the first / that is present in the path
        lock_file_index.remove(0);
    }

    // verify that
    // Normalize possible trailing slash differences
    if !local_pypi_index.ends_with('/') {
        local_pypi_index.push('/');
    }
    if !lock_file_index.ends_with('/') {
        lock_file_index.push('/');
    }
    assert_eq!(local_pypi_index, lock_file_index,);
}

/// Ensures the unsafe-best-match index strategy is honored when resolving and building PyPI projects,
/// even when the lower version appears first in `extra-index-urls`.
/// This was an issue in: https://github.com/prefix-dev/pixi/issues/4588
#[tokio::test]
#[cfg_attr(
    any(not(feature = "online_tests"), not(feature = "slow_integration_tests")),
    ignore
)]
async fn test_index_strategy_respected_for_build_dependencies() {
    setup_tracing();

    // The first extra index exposes the lower version while the second extra exposes the higher
    // one. `unsafe-best-match` should still select the best version even though the lower version
    // is encountered earlier.
    let first_extra_index = PyPIDatabase::new()
        .with(PyPIPackage::new("foozy", "1.0.0"))
        .into_simple_index()
        .unwrap();
    let second_extra_index = PyPIDatabase::new()
        .with(PyPIPackage::new("foozy", "2.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_pyproject_manifest(&format!(
        r#"
        [project]
        name = "index-strategy-build"
        requires-python = ">=3.10"
        version = "0.1.0"

        [build-system]
        requires = [
            "uv_build>=0.8.9,<0.9.0",
            "foozy==2.0.0",
        ]
        build-backend = "uv_build"

        [tool.pixi.workspace]
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["{platform}"]

        [tool.pixi.dependencies]
        python = "~=3.12.0"

        [tool.pixi.pypi-options]
        extra-index-urls = [
            "{first_extra_index}",
            "{second_extra_index}",
        ]
        # Without this the test will fail
        index-strategy = "unsafe-best-match"

        [tool.pixi.pypi-dependencies]
        index-strategy-build = {{ path = ".", editable = true }}
        "#,
        platform = Platform::current(),
        first_extra_index = first_extra_index.index_url(),
        second_extra_index = second_extra_index.index_url(),
    ))
    .unwrap();

    let project_path = pixi.workspace_path();
    let src_dir = project_path.join("src").join("index_strategy_build");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "").unwrap();

    pixi.install().await.unwrap();
}

#[tokio::test]
async fn test_cross_platform_resolve_with_no_build() {
    setup_tracing();

    // non-current platform
    let resolve_platform = if Platform::current().is_osx() {
        Platform::Linux64
    } else {
        Platform::OsxArm64
    };

    // Create local conda channel with Python for the resolve platform
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(resolve_platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Use a local flat index for foo==1.0.0
    let flat = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_flat_index()
        .expect("failed to create flat index");
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        no-build = true
        find-links = [{{ path = "{find_links}"}}]"#,
        platform = resolve_platform,
        channel_url = channel.url(),
        find_links = flat.path().display().to_string().replace("\\", "/"),
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", resolve_platform, "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*flat.path().as_os_str().to_string_lossy())
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

/// This test checks that the help message is correctly generated when a PyPI package is pinned
/// by the conda solve, which may cause a conflict with the PyPI dependencies.
///
/// We expect there to be a help message that informs the user about the pinned package
#[tokio::test]
async fn test_pinned_help_message() {
    setup_tracing();

    // Construct a minimal local conda channel with python and pandas==1.0.0
    use rattler_conda_types::Platform;

    let mut conda_db = MockRepoData::default();
    // Python runtime
    conda_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    // pandas 1.0.0 (marked as PyPI package via purl)
    conda_db.add_package(
        Package::build("pandas", "1.0.0")
            .with_subdir(Platform::current())
            .with_dependency("python >=3.12")
            .with_pypi_purl("pandas")
            .finish(),
    );
    let conda_channel = conda_db.into_channel().await.unwrap();

    // Build a simple PyPI index with package `a` that requires pandas>=2.0.0
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("a", "1.0.0").with_requires_dist(["pandas>=2.0.0"]))
        .into_simple_index()
        .unwrap();

    // Use only our local channel and local simple index
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel}"]
        conda-pypi-map = {{}}
        name = "local-pinned-help"
        platforms = ["{platform}"]
        version = "0.1.0"

        [dependencies]
        python = "3.12.*"
        pandas = "==1.0.0"

        [pypi-dependencies]
        a = "*"

        [pypi-options]
        extra-index-urls = ["{idx}"]
        "#,
        channel = conda_channel.url(),
        platform = Platform::current(),
        idx = pypi_index.index_url(),
    ));

    // Expect failure
    let result = pixi.unwrap().update_lock_file().await;
    let err = result.expect_err("expected a resolution error");
    // Should contain pinned help message for pandas==1.0.0
    assert_eq!(
        format!("{}", err.help().unwrap()),
        "The following PyPI packages have been pinned by the conda solve, and this version may be causing a conflict:\npandas==1.0.0
See https://pixi.sh/latest/concepts/conda_pypi/#pinned-package-conflicts for more information."
    );
}

#[tokio::test]
async fn test_uv_index_correctly_parsed() {
    setup_tracing();

    let platform = Platform::current();

    // Create local conda channel with Python
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();

    // Provide a local simple index containing `foo` used in build-system requires.
    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .expect("failed to create simple index");

    let pixi = PixiControl::from_pyproject_manifest(&format!(
        r#"
        [project]
        name = "simple"
        version = "0.1.0"
        requires-python = ">=3.11"
        dependencies = ["foo"]

        [build-system]
        requires = ["uv_build>=0.8.9,<0.9.0"]
        build-backend = "uv_build"


        [tool.uv.sources]
        foo = [
        {{ index = "our_index" }},
        ]

        [[tool.uv.index]]
        name = "our_index"
        url = "{index_url}"
        explicit = true

        [tool.uv.build-backend]
        module-name = "simple"
        module-root = ""

        [tool.pixi.workspace]
        channels = ["{channel_url}"]
        platforms = ["{platform}"]
        conda-pypi-map = {{}} # Disable mapping

        [tool.pixi.pypi-dependencies]
        simple = {{ path = "." }}
        "#,
        platform = platform,
        channel_url = channel.url(),
        index_url = simple.index_url(),
    ))
    .unwrap();

    let project_path = pixi.workspace_path();
    let src_dir = project_path.join("src").join("simple");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "").unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();
    assert!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap()
            .as_str()
            .contains(&simple.index_path().display().to_string())
    );
}

/// Tests that prerelease-mode = "allow" allows pre-release versions to be resolved.
/// Without this setting, the resolver would skip pre-releases unless explicitly requested.
#[tokio::test]
async fn test_prerelease_mode_allow() {
    setup_tracing();

    // Build a local simple index with both a stable and prerelease version
    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("testpkg", "1.0.0"))
        .with(PyPIPackage::new("testpkg", "2.0.0a1")) // Pre-release version
        .into_simple_index()
        .expect("failed to create local simple index");

    let platform = Platform::current();

    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();
    let channel_url = channel.url();

    // With prerelease-mode = "allow", the resolver should pick the pre-release 2.0.0a1
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "prerelease-test"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}} # Disable mapping

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        testpkg = "*"

        [pypi-options]
        index-url = "{index_url}"
        prerelease-mode = "allow"
        "#,
        platform = platform,
        channel_url = channel_url,
        index_url = simple.index_url(),
    ))
    .unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    // With prerelease-mode = "allow", we should get the pre-release version 2.0.0a1
    // because it's the highest version available
    let locked_version = lock_file
        .get_pypi_package_version("default", platform, "testpkg")
        .expect("testpkg should be in lock file");
    assert_eq!(
        locked_version.to_string(),
        "2.0.0a1",
        "With prerelease-mode = 'allow', the pre-release version should be selected"
    );
}

/// Tests that prerelease-mode = "disallow" prevents pre-release versions from being resolved.
#[tokio::test]
async fn test_prerelease_mode_disallow() {
    setup_tracing();

    // Build a local simple index with both a stable and prerelease version
    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("testpkg", "1.0.0"))
        .with(PyPIPackage::new("testpkg", "2.0.0a1")) // Pre-release version
        .into_simple_index()
        .expect("failed to create local simple index");

    let platform = Platform::current();

    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();
    let channel_url = channel.url();

    // With prerelease-mode = "disallow", the resolver should pick the stable 1.0.0
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        name = "prerelease-test"
        platforms = ["{platform}"]
        channels = ["{channel_url}"]
        conda-pypi-map = {{}}

        [dependencies]
        python = "==3.12.0"

        [pypi-dependencies]
        testpkg = "*"

        [pypi-options]
        index-url = "{index_url}"
        prerelease-mode = "disallow"
        "#,
        platform = platform,
        channel_url = channel_url,
        index_url = simple.index_url(),
    ))
    .unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    // With prerelease-mode = "disallow", we should get the stable version 1.0.0
    let locked_version = lock_file
        .get_pypi_package_version("default", platform, "testpkg")
        .expect("testpkg should be in lock file");
    assert_eq!(
        locked_version.to_string(),
        "1.0.0",
        "With prerelease-mode = 'disallow', the stable version should be selected"
    );
}

/// Test that PyPI sdist with static metadata (all in pyproject.toml) can be resolved.
/// This tests the satisfiability check extracts metadata without running setup.py.
#[tokio::test]
#[cfg_attr(
    any(not(feature = "online_tests"), not(feature = "slow_integration_tests")),
    ignore
)]
async fn test_pypi_sdist_static_metadata_extraction() {
    setup_tracing();

    let platform = Platform::current();

    // Create a pyproject.toml with all static metadata (hatchling build backend)
    let pyproject = format!(
        r#"
[project]
name = "test-static-pkg"
version = "1.2.3"
description = "Test package with static metadata"
requires-python = ">=3.10"
dependencies = []

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build]
include = ["src"]
targets.wheel.strict-naming = false
targets.wheel.packages = ["src/test_static_pkg"]
targets.sdist.strict-naming = false
targets.sdist.packages = ["src/test_static_pkg"]

[tool.pixi.workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{platform}"]

[tool.pixi.dependencies]
python = "~=3.12.0"

[tool.pixi.pypi-dependencies]
test-static-pkg = {{ path = ".", editable = true }}
"#,
    );

    let pixi = PixiControl::from_pyproject_manifest(&pyproject).unwrap();

    // Create the package source files
    let src_dir = pixi.workspace_path().join("src").join("test_static_pkg");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "__version__ = '1.2.3'\n").unwrap();

    // Resolve the lock file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify the package is in the lock file with correct version
    let locked_version = lock_file
        .get_pypi_package_version("default", platform, "test-static-pkg")
        .expect("test-static-pkg should be in lock file");

    assert_eq!(
        locked_version.to_string(),
        "1.2.3",
        "Static metadata version should be correctly extracted"
    );
}

/// Test that PyPI sdist with dynamic metadata (version from setup.py) can be resolved.
/// This tests the satisfiability check runs the build backend to extract metadata.
#[tokio::test]
#[cfg_attr(
    any(not(feature = "online_tests"), not(feature = "slow_integration_tests")),
    ignore
)]
async fn test_pypi_sdist_dynamic_metadata_extraction() {
    setup_tracing();

    let platform = Platform::current();

    // Create a pyproject.toml with dynamic version (setuptools)
    let pyproject = format!(
        r#"
[project]
name = "test-dynamic-pkg"
dynamic = ["version"]
requires-python = ">=3.10"
dependencies = []

[build-system]
requires = ["setuptools>=61.0"]
build-backend = "setuptools.build_meta"

[tool.setuptools.dynamic]
version = {{attr = "test_dynamic_pkg.__version__"}}

[tool.pixi.workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{platform}"]

[tool.pixi.dependencies]
python = "~=3.12.0"

[tool.pixi.pypi-dependencies]
test-dynamic-pkg = {{ path = ".", editable = true }}
"#,
    );

    let pixi = PixiControl::from_pyproject_manifest(&pyproject).unwrap();

    // Create the package source files with version defined in __init__.py
    let pkg_dir = pixi.workspace_path().join("test_dynamic_pkg");
    fs_err::create_dir_all(&pkg_dir).unwrap();
    fs_err::write(pkg_dir.join("__init__.py"), "__version__ = '2.5.0'\n").unwrap();

    // Resolve the lock file
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Verify the package is in the lock file with dynamically extracted version
    let locked_version = lock_file
        .get_pypi_package_version("default", platform, "test-dynamic-pkg")
        .expect("test-dynamic-pkg should be in lock file");

    assert_eq!(
        locked_version.to_string(),
        "2.5.0",
        "Dynamic metadata version should be correctly extracted"
    );
}

/// Find all sdist cache directories under uv-cache/sdists-v*/path/
fn find_sdist_cache_dirs(cache_dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let uv_cache = cache_dir.join("uv-cache");
    if !uv_cache.exists() {
        return result;
    }

    // Look for sdists-v* directories (e.g., sdists-v9)
    if let Ok(entries) = fs_err::read_dir(&uv_cache) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("sdists-v") {
                    let path_dir = path.join("path");
                    if path_dir.exists()
                        && let Ok(hash_dirs) = fs_err::read_dir(&path_dir)
                    {
                        for hash_entry in hash_dirs.filter_map(|e| e.ok()) {
                            let hash_path = hash_entry.path();
                            if hash_path.is_dir() {
                                result.push(hash_path);
                            }
                        }
                    }
                }
            }
        }
    }
    result
}

/// Test that PyPI sdist builds are cached and reused on subsequent installs.
/// Uses file modification time verification to check no rebuild occurred.
#[tokio::test]
#[cfg_attr(
    any(not(feature = "online_tests"), not(feature = "slow_integration_tests")),
    ignore
)]
async fn test_pypi_sdist_cache_reuse() {
    setup_tracing();

    let platform = Platform::current();

    let pyproject = format!(
        r#"
[project]
name = "test-cache-pkg"
version = "1.0.0"
requires-python = ">=3.10"
dependencies = []

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build]
include = ["src"]
targets.wheel.strict-naming = false
targets.wheel.packages = ["src/test_cache_pkg"]
targets.sdist.strict-naming = false
targets.sdist.packages = ["src/test_cache_pkg"]

[tool.pixi.workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{platform}"]

[tool.pixi.dependencies]
python = "~=3.12.0"

[tool.pixi.pypi-dependencies]
test-cache-pkg = {{ path = "." }}
"#,
    );

    let pixi = PixiControl::from_pyproject_manifest(&pyproject).unwrap();

    // Create the package source files
    let src_dir = pixi.workspace_path().join("src").join("test_cache_pkg");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "__version__ = '1.0.0'\n").unwrap();

    // First install - builds and caches the wheel
    let tmp_dir = tempdir().unwrap();
    let cache_dir = tmp_dir.path().to_path_buf();

    temp_env::async_with_vars(
        [("PIXI_CACHE_DIR", Some(tmp_dir.path().to_str().unwrap()))],
        async {
            pixi.install().await.unwrap();
        },
    )
    .await;

    // Find the sdist cache directory for our package
    let sdist_dirs = find_sdist_cache_dirs(&cache_dir);
    assert!(
        !sdist_dirs.is_empty(),
        "Expected sdist cache directory to exist after first install"
    );

    // Record mtimes of all sdist cache directories
    let mtimes_first: HashMap<PathBuf, SystemTime> = sdist_dirs
        .iter()
        .filter_map(|p| {
            fs_err::metadata(p)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (p.clone(), t))
        })
        .collect();

    // Second install - should reuse cache without rebuilding
    temp_env::async_with_vars(
        [("PIXI_CACHE_DIR", Some(tmp_dir.path().to_str().unwrap()))],
        async {
            pixi.install().await.unwrap();
        },
    )
    .await;

    // Verify sdist cache directories were not modified (no rebuild occurred)
    for (path, mtime_first) in &mtimes_first {
        let mtime_second = fs_err::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| panic!("Cache directory disappeared: {}", path.display()));
        assert_eq!(
            *mtime_first,
            mtime_second,
            "Sdist cache was modified (rebuilt): {}",
            path.display()
        );
    }
}

/// Test for issue #5205: Specifying a python sub-version (patch) should work correctly
/// Before the fix, using python 3.10.6 would create a specifier "==3.10.*" which conflicts
/// with requires-python = "==3.10.6". The fix uses the full version string.
#[tokio::test]
async fn test_python_patch_version_requires_python() {
    setup_tracing();

    let platform = Platform::current();

    // Test with different requires-python formats to ensure robustness
    let test_cases = vec![("==3.10.6", true), (">=3.11", false), ("==3.7.2", true)];

    // Create local conda channel with Python 3.10.6 (with patch version)
    let mut package_db = MockRepoData::default();
    package_db.add_package(
        Package::build("python", "3.10.6")
            .with_subdir(platform)
            .finish(),
    );
    let channel = package_db.into_channel().await.unwrap();
    let channel_url = channel.url();

    for (requires_python, should_solve) in test_cases {
        // Create a pyproject.toml with requires-python
        let pyproject = format!(
            r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "test-project"
version = "0.1.0"
requires-python = "{requires_python}"

[tool.pixi.workspace]
channels = ["{channel_url}"]
platforms = ["{platform}"]
conda-pypi-map = {{}}

[tool.pixi.dependencies]
python = "==3.10.6"

[tool.pixi.pypi-dependencies]
test-project = {{ path = "." }}
"#,
            channel_url = channel_url,
            platform = platform,
            requires_python = requires_python,
        );

        let pixi = PixiControl::from_pyproject_manifest(&pyproject).unwrap();

        let result = pixi.update_lock_file().await;

        assert_eq!(
            result.is_ok(),
            should_solve,
            "Expected solving to be {} for requires-python = '{}'",
            if should_solve {
                "successful"
            } else {
                "unsuccessful"
            },
            requires_python,
        );

        // Verify that the lock file was created successfully and test-project was resolved
        if let Ok(lock_file) = result {
            let test_project_version =
                lock_file.get_pypi_package_version("default", platform, "test-project");
            assert!(
                test_project_version.is_some(),
                "test-project should be resolved for requires-python = '{}'",
                requires_python
            );
        }
    }
}
