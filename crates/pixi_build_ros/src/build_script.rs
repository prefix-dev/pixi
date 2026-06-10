//! Build script template selection and variable substitution.

use std::path::Path;

use miette::Diagnostic;
use rattler_conda_types::Platform;
use thiserror::Error;

use crate::ament_python::{self, AmentPythonFlavor};

/// Errors that can occur during build script generation.
#[derive(Debug, Error, Diagnostic)]
pub enum BuildScriptError {
    #[error("unsupported ROS build type: '{build_type}'")]
    #[diagnostic(help("Supported build types are: ament_cmake, ament_python, cmake, catkin"))]
    UnsupportedBuildType { build_type: String },

    #[error(transparent)]
    #[diagnostic(transparent)]
    AmentPython(#[from] ament_python::AmentPythonError),
}

/// Render a build script from the appropriate template.
///
/// Selects the template based on `build_type` and platform, then performs
/// variable substitution. For `ament_python` packages without a `setup.py`
/// but with a `pyproject.toml`, a pyproject-specific template is used that
/// registers the package in the ament resource index manually.
pub fn render_build_script(
    build_type: &str,
    distro: &str,
    source_dir: &Path,
    package_name: &str,
) -> Result<String, BuildScriptError> {
    // Use the current (build) platform, not the host/target platform.
    // The build script runs on the build machine.
    let is_windows = Platform::current().is_windows();
    let mut template = select_template(build_type, is_windows)?.to_string();

    if build_type == "ament_python"
        && ament_python::detect_flavor(source_dir) == AmentPythonFlavor::Pyproject
    {
        template = if is_windows {
            include_str!("../templates/bld_ament_python_pyproject.bat")
        } else {
            include_str!("../templates/build_ament_python_pyproject.sh")
        }
        .to_string();

        let scripts = ament_python::entry_points(&source_dir.join("pyproject.toml"))?;
        let install_lines =
            ament_python::entry_point_install_lines(package_name, &scripts, is_windows);
        template = template
            .replace("@ROS_PKG_NAME@", package_name)
            .replace("@ENTRY_POINT_INSTALL@", &install_lines);
    }

    let src_dir_str = source_dir.display().to_string();
    let rendered = template
        .replace("@SRC_DIR@", &src_dir_str)
        .replace("@DISTRO@", distro)
        .replace("@BUILD_DIR@", "build")
        .replace("@BUILD_TYPE@", "Release");

    Ok(rendered)
}

fn select_template(build_type: &str, is_windows: bool) -> Result<&'static str, BuildScriptError> {
    match (build_type, is_windows) {
        ("ament_cmake", false) => Ok(include_str!("../templates/build_ament_cmake.sh")),
        ("ament_cmake", true) => Ok(include_str!("../templates/bld_ament_cmake.bat")),
        ("ament_python", false) => Ok(include_str!("../templates/build_ament_python.sh")),
        ("ament_python", true) => Ok(include_str!("../templates/bld_ament_python.bat")),
        ("cmake" | "catkin", false) => Ok(include_str!("../templates/build_catkin.sh")),
        ("cmake" | "catkin", true) => Ok(include_str!("../templates/bld_catkin.bat")),
        _ => Err(BuildScriptError::UnsupportedBuildType {
            build_type: build_type.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_render_ament_cmake() {
        let script = render_build_script(
            "ament_cmake",
            "humble",
            &PathBuf::from("/my/source"),
            "my_package",
        )
        .unwrap();

        assert!(script.contains("/my/source"));
        assert!(script.contains("Release"));
        assert!(!script.contains("@SRC_DIR@"));
        assert!(!script.contains("@BUILD_TYPE@"));
    }

    #[test]
    fn test_render_ament_python() {
        let script = render_build_script(
            "ament_python",
            "jazzy",
            &PathBuf::from("/src"),
            "my_package",
        )
        .unwrap();

        assert!(script.contains("/src"));
        assert!(!script.contains("@SRC_DIR@"));
    }

    #[test]
    fn test_render_ament_python_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(
            dir.path().join("pyproject.toml"),
            r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "my_package"
version = "0.1.0"

[project.scripts]
navigate = "my_package.navigator:main"
"#,
        )
        .unwrap();

        let script =
            render_build_script("ament_python", "jazzy", dir.path(), "my_package").unwrap();

        assert!(script.contains("--no-build-isolation"));
        assert!(!script.contains("@ROS_PKG_NAME@"));
        assert!(!script.contains("@ENTRY_POINT_INSTALL@"));
        if cfg!(windows) {
            assert!(script.contains(
                r"%LIBRARY_PREFIX%\share\ament_index\resource_index\packages\my_package"
            ));
            assert!(script.contains(r#"copy "%PREFIX%\Scripts\navigate*""#));
        } else {
            assert!(
                script.contains("$PREFIX/share/ament_index/resource_index/packages/my_package")
            );
            assert!(script.contains("cp package.xml \"$PREFIX/share/my_package/package.xml\""));
            assert!(
                script.contains("cp \"$PREFIX/bin/navigate\" \"$PREFIX/lib/my_package/navigate\"")
            );
        }
    }

    #[test]
    fn test_render_ament_python_setup_py_wins() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("setup.py"), "").unwrap();
        fs_err::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"my_package\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let script =
            render_build_script("ament_python", "jazzy", dir.path(), "my_package").unwrap();

        assert!(!script.contains("--no-build-isolation"));
        assert!(script.contains("setup.py") || script.contains("setup.cfg"));
    }

    #[test]
    fn test_render_catkin() {
        let script =
            render_build_script("catkin", "noetic", &PathBuf::from("/pkg"), "my_package").unwrap();

        assert!(script.contains("/pkg"));
        assert!(script.contains("noetic"));
    }

    #[test]
    fn test_unsupported_build_type() {
        let result = render_build_script(
            "unknown_type",
            "jazzy",
            &PathBuf::from("/src"),
            "my_package",
        );
        assert!(matches!(
            result,
            Err(BuildScriptError::UnsupportedBuildType { .. })
        ));
    }
}
