use std::io::Write;

use rattler_conda_types::Platform;
use typed_path::Utf8TypedPath;

use crate::common::pypi_index::{Database as PyPIDatabase, PyPIPackage};
use crate::common::{LockFileExt, PixiControl};
use crate::setup_tracing;
use std::fs::File;

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_flat_links_based_index_returns_path() {
    setup_tracing();

    // Build a local flat (find-links) index with a single wheel: foo==1.0.0
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_flat_index()
        .expect("failed to create local flat index");

    let find_links_path = index.path().display().to_string().replace('\\', "/");

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-flat-find-links"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        find-links = [{{ path = "{find_links_path}"}}]
        "#,
        platform = Platform::current(),
        find_links_path = find_links_path,
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    // Expect the locked URL to be a local path pointing at our generated wheel.
    // Our wheel builder uses the tag py3-none-any by default.
    assert_eq!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*index.path().as_os_str().to_string_lossy())
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_file_based_index_returns_path() {
    setup_tracing();

    let simple = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .expect("failed to create simple index");

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        extra-index-urls = [
            "{index_url}"
        ]"#,
        platform = Platform::current(),
        index_url = simple.index_url(),
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*simple.index_path().as_os_str().to_string_lossy())
            .join("foo")
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_index_strategy() {
    setup_tracing();

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
        [project]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

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
        platform = Platform::current(),
        idx_a = idx_a.index_url(),
        idx_b = idx_b.index_url(),
        idx_c = idx_c.index_url(),
    ));

    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file.get_pypi_package_version("default", Platform::current(), "foo"),
        Some("1.0.0".into())
    );
    assert_eq!(
        lock_file.get_pypi_package_version(
            "unsafe-first-match-unconstrained",
            Platform::current(),
            "foo"
        ),
        Some("1.0.0".into())
    );

    assert_eq!(
        lock_file.get_pypi_package_version(
            "unsafe-first-match-constrained",
            Platform::current(),
            "foo"
        ),
        Some("3.0.0".into())
    );
    assert_eq!(
        lock_file.get_pypi_package_version("unsafe-best-match", Platform::current(), "foo"),
        Some("3.0.0".into())
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
/// This test checks if we can pin a package from a PyPI index, by explicitly specifying the index.
async fn test_pinning_index() {
    setup_tracing();

    let idx = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_simple_index()
        .unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-pinning-index"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [pypi-dependencies]
        foo = {{ version = "*", index = "{idx_url}" }}

        "#,
        platform = Platform::current(),
        idx_url = idx.index_url(),
    ));

    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*idx.index_path().as_os_str().to_string_lossy())
            .join("foo")
            .join("foo-1.0.0-py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
/// This test checks if we can receive torch correctly from the whl/cu124 index.
async fn pin_torch() {
    setup_tracing();

    // Do some platform magic, as the index does not contain wheels for each platform.
    let platform = Platform::current();
    let platforms = match platform {
        Platform::Linux64 => "\"linux-64\"".to_string(),
        _ => format!("\"{platform}\", \"linux-64\"", platform = platform),
    };

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-pinning-index"
        platforms = [{platforms}]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [target.linux-64.pypi-dependencies]
        torch = {{ version = "*", index = "https://download.pytorch.org/whl/cu124" }}
        "#,
        platforms = platforms,
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
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_allow_insecure_host() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [pypi-dependencies]
        sh = "*"

        [pypi-options]
        extra-index-urls = ["https://expired.badssl.com/"]"#,
        platform = Platform::current(),
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
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
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



        [tool.pixi.project]
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
    pixi.install().await.unwrap();

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

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_cross_platform_resolve_with_no_build() {
    setup_tracing();

    // non-current platform
    let resolve_platform = if Platform::current().is_osx() {
        Platform::Linux64
    } else {
        Platform::OsxArm64
    };

    // Use a local flat index for foo==1.0.0
    let flat = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "1.0.0"))
        .into_flat_index()
        .expect("failed to create flat index");
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-extra-index-url"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [pypi-dependencies]
        foo = "*"

        [pypi-options]
        no-build = true
        find-links = [{{ path = "{find_links}"}}]"#,
        platform = resolve_platform,
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
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_pinned_help_message() {
    setup_tracing();

    // Construct a minimal local conda channel with python and pandas==1.0.0
    use crate::common::package_database::{Package, PackageDatabase};
    use rattler_conda_types::Platform;

    let mut conda_db = PackageDatabase::default();
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
        "The following PyPI packages have been pinned by the conda solve, and this version may be causing a conflict:\npandas==1.0.0"
    );
}
