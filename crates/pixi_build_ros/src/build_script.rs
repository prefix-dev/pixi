//! Build script template selection and variable substitution.

use std::path::Path;

use miette::Diagnostic;
use rattler_conda_types::Subdir;
use thiserror::Error;

/// Errors that can occur during build script generation.
#[derive(Debug, Error, Diagnostic)]
pub enum BuildScriptError {
    #[error("unsupported ROS build type: '{build_type}'")]
    #[diagnostic(help("Supported build types are: ament_cmake, ament_python, cmake, catkin"))]
    UnsupportedBuildType { build_type: String },
}

/// Render a build script from the appropriate template.
///
/// Selects the template based on `build_type` and platform, then performs
/// variable substitution.
pub fn render_build_script(
    build_type: &str,
    distro: &str,
    source_dir: &Path,
) -> Result<String, BuildScriptError> {
    // Use the current (build) platform, not the host/target platform.
    // The build script runs on the build machine.
    let is_windows = Subdir::current().unwrap_or(Subdir::NoArch).is_windows();
    let template = select_template(build_type, is_windows)?;

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
        let script =
            render_build_script("ament_cmake", "humble", &PathBuf::from("/my/source")).unwrap();

        assert!(script.contains("/my/source"));
        assert!(script.contains("Release"));
        assert!(!script.contains("@SRC_DIR@"));
        assert!(!script.contains("@BUILD_TYPE@"));
    }

    #[test]
    fn test_windows_ament_cmake_uses_absolute_python_install_dir() {
        // Regression test: a relative PYTHON_INSTALL_DIR such as
        // "../Lib/site-packages" is recorded literally by CMake in
        // install_manifest.txt and then leaks into the package file list, where
        // entries containing `..` are rejected.
        let template = include_str!("../templates/bld_ament_cmake.bat");
        assert!(
            template.contains(r#"set "PYTHON_INSTALL_DIR=%SP_DIR:\=/%""#),
            "the ament_cmake Windows template must use an absolute site-packages path"
        );
        assert!(
            !template.contains("relpath"),
            "the relative PYTHON_INSTALL_DIR computation must be gone"
        );
    }

    #[test]
    fn test_windows_ament_cmake_quotes_path_arguments() {
        // cmd.exe splits an unquoted expanded value on spaces, so the build
        // prefix must not be handed to cmake as a bare %VAR%.
        let template = include_str!("../templates/bld_ament_cmake.bat");
        for arg in [
            r#""-DCMAKE_INSTALL_PREFIX=%LIBRARY_PREFIX%""#,
            r#""-DPYTHON_EXECUTABLE=%PYTHON%""#,
            r#""-DPYTHON_INSTALL_DIR=%PYTHON_INSTALL_DIR%""#,
            r#""%SRC_DIR%""#,
        ] {
            assert!(template.contains(arg), "expected quoted argument {arg}");
        }
    }

    #[test]
    fn test_render_ament_python() {
        let script = render_build_script("ament_python", "jazzy", &PathBuf::from("/src")).unwrap();

        assert!(script.contains("/src"));
        assert!(!script.contains("@SRC_DIR@"));
    }

    #[test]
    fn test_render_catkin() {
        let script = render_build_script("catkin", "noetic", &PathBuf::from("/pkg")).unwrap();

        assert!(script.contains("/pkg"));
        assert!(script.contains("noetic"));
    }

    #[test]
    fn test_unsupported_build_type() {
        let result = render_build_script("unknown_type", "jazzy", &PathBuf::from("/src"));
        assert!(matches!(
            result,
            Err(BuildScriptError::UnsupportedBuildType { .. })
        ));
    }
}
