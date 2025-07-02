use std::{io::Write, path::Path};

use rattler_conda_types::Platform;
use typed_path::Utf8TypedPath;
use url::Url;

use crate::common::{LockFileExt, PixiControl};
use std::fs::File;

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_flat_links_based_index_returns_path() {
    let pypi_indexes = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pypi-indexes");
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
        find-links = [{{ path = "{pypi_indexes}/multiple-indexes-a/flat"}}]"#,
        platform = Platform::current(),
        pypi_indexes = pypi_indexes.display().to_string().replace("\\", "/"),
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    // This assertion is specifically to test that if we have a url-based *local* index
    // we will get a path back to the index and the corresponding file
    assert_eq!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*pypi_indexes.as_os_str().to_string_lossy())
            .join("multiple-indexes-a")
            .join("flat")
            .join("foo-1.0.0-py2.py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_file_based_index_returns_path() {
    let pypi_indexes = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pypi-indexes");
    let pypi_indexes_url = Url::from_directory_path(pypi_indexes.clone()).unwrap();
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
            "{pypi_indexes}multiple-indexes-a/index"
        ]"#,
        platform = Platform::current(),
        pypi_indexes = pypi_indexes_url,
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    // This assertion is specifically to test that if we have a url-based *local* index
    // we will get a path back to the index and the corresponding file
    assert_eq!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*pypi_indexes.as_os_str().to_string_lossy())
            .join("multiple-indexes-a/index/foo")
            .join("foo-1.0.0-py2.py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_index_strategy() {
    let pypi_indexes = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pypi-indexes");
    let pypi_indexes_url = Url::from_directory_path(pypi_indexes.clone()).unwrap();

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
            "{pypi_indexes}multiple-indexes-a/index",
            "{pypi_indexes}multiple-indexes-b/index",
            "{pypi_indexes}multiple-indexes-c/index",
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
        pypi_indexes = pypi_indexes_url,
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
    let pypi_indexes = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pypi-indexes");
    let pypi_indexes_url = Url::from_directory_path(pypi_indexes.clone()).unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "pypi-pinning-index"
        platforms = ["{platform}"]
        channels = ["https://prefix.dev/conda-forge"]

        [dependencies]
        python = "~=3.12.0"

        [pypi-dependencies]
        foo = {{ version = "*", index = "{pypi_indexes}multiple-indexes-a/index" }}

        "#,
        platform = Platform::current(),
        pypi_indexes = pypi_indexes_url,
    ));

    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", Platform::current(), "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*pypi_indexes.as_os_str().to_string_lossy())
            .join("multiple-indexes-a/index/foo")
            .join("foo-1.0.0-py2.py3-none-any.whl")
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
/// This test checks if we can receive torch correctly from the whl/cu124 index.
async fn pin_torch() {
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
    let pypi_indexes = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pypi-indexes");
    let pypi_indexes_url = Url::from_directory_path(pypi_indexes.clone()).unwrap();

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
        index-url = "{pypi_indexes}multiple-indexes-a/index"
        no-build-isolation = ["pypi-build-index"]

        [tool.pixi.pypi-dependencies]
        pypi-build-index = {{ path = ".", editable = true }}
        "#,
        platform = Platform::current(),
        pypi_indexes = pypi_indexes_url,
    ))
    .unwrap();

    let project_path = pixi.workspace_path();
    let src_dir = project_path.join("src").join("pypi_build_index");
    fs_err::create_dir_all(&src_dir).unwrap();
    fs_err::write(src_dir.join("__init__.py"), "").unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    // verify that the pypi-build-index can be installed when solved the build dependencies
    pixi.install().await.unwrap();

    let mut local_pypi_index = pypi_indexes
        .join("multiple-indexes-a")
        .join("index")
        .display()
        .to_string();

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
    assert_eq!(local_pypi_index, lock_file_index,);
}

#[tokio::test]
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_cross_platform_resolve_with_no_build() {
    // non-current platform
    let resolve_platform = if Platform::current().is_osx() {
        Platform::Linux64
    } else {
        Platform::OsxArm64
    };

    let pypi_indexes = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/pypi-indexes");
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
        find-links = [{{ path = "{pypi_indexes}/multiple-indexes-a/flat"}}]"#,
        platform = resolve_platform,
        pypi_indexes = pypi_indexes.display().to_string().replace("\\", "/"),
    ));
    let lock_file = pixi.unwrap().update_lock_file().await.unwrap();

    assert_eq!(
        lock_file
            .get_pypi_package_url("default", resolve_platform, "foo")
            .unwrap()
            .as_path()
            .unwrap(),
        Utf8TypedPath::from(&*pypi_indexes.as_os_str().to_string_lossy())
            .join("multiple-indexes-a")
            .join("flat")
            .join("foo-1.0.0-py2.py3-none-any.whl")
    );
}
