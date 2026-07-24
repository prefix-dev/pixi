//! Build script template selection and variable substitution.

use std::path::Path;

use miette::Diagnostic;
use rattler_conda_types::Platform;
use thiserror::Error;

/// Errors that can occur during build script generation.
#[derive(Debug, Error, Diagnostic)]
pub enum BuildScriptError {
    #[error("unsupported ROS build type: '{build_type}'")]
    #[diagnostic(help("Supported build types are: ament_cmake, ament_python, cmake, catkin"))]
    UnsupportedBuildType { build_type: String },
}

/// A rendered build script and the interpreter it should run under.
pub struct RenderedBuildScript {
    /// The script contents with all placeholders substituted.
    pub content: String,
    /// The interpreter to run the script with (e.g. `"python"`), or `None` to
    /// let rattler-build use the platform default shell.
    pub interpreter: Option<&'static str>,
}

/// Render a build script from the appropriate template.
///
/// Selects the template based on `build_type` and platform, then performs
/// variable substitution. The `ament_python` template is a single Python
/// script that handles both `setup.py` and `pyproject.toml` based packages: it
/// installs the package with pip, registers it in the ament resource index,
/// and copies the console scripts into `lib/<package>` where `ros2 run` looks
/// for them.
pub fn render_build_script(
    build_type: &str,
    distro: &str,
    source_dir: &Path,
    package_name: &str,
) -> Result<RenderedBuildScript, BuildScriptError> {
    // Use the current (build) platform, not the host/target platform.
    // The build script runs on the build machine.
    let is_windows = Platform::current().is_windows();
    let (template, interpreter) = select_template(build_type, is_windows)?;

    let src_dir_str = source_dir.display().to_string();
    let content = template
        .replace("@SRC_DIR@", &src_dir_str)
        .replace("@DISTRO@", distro)
        .replace("@ROS_PKG_NAME@", package_name)
        .replace("@BUILD_DIR@", "build")
        .replace("@BUILD_TYPE@", "Release");

    Ok(RenderedBuildScript {
        content,
        interpreter,
    })
}

/// Returns the template contents and the interpreter it must run under.
fn select_template(
    build_type: &str,
    is_windows: bool,
) -> Result<(&'static str, Option<&'static str>), BuildScriptError> {
    match (build_type, is_windows) {
        // A single cross-platform Python script handles ament_python.
        ("ament_python", _) => Ok((include_str!("../templates/build_ament_python.py"), Some("python"))),
        ("ament_cmake", false) => Ok((include_str!("../templates/build_ament_cmake.sh"), None)),
        ("ament_cmake", true) => Ok((include_str!("../templates/bld_ament_cmake.bat"), None)),
        ("cmake" | "catkin", false) => Ok((include_str!("../templates/build_catkin.sh"), None)),
        ("cmake" | "catkin", true) => Ok((include_str!("../templates/bld_catkin.bat"), None)),
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

        assert!(script.content.contains("/my/source"));
        assert!(script.content.contains("Release"));
        assert!(!script.content.contains("@SRC_DIR@"));
        assert!(!script.content.contains("@BUILD_TYPE@"));
        assert_eq!(script.interpreter, None);
    }

    #[test]
    fn test_render_ament_python() {
        let script =
            render_build_script("ament_python", "jazzy", &PathBuf::from("/src"), "my_package")
                .unwrap();

        // The ament_python flow is a single Python script run under the python
        // interpreter, identical on every platform.
        assert_eq!(script.interpreter, Some("python"));
        assert!(script.content.contains("/src"));
        assert!(!script.content.contains("@SRC_DIR@"));
        assert!(!script.content.contains("@ROS_PKG_NAME@"));
        assert!(script.content.contains("PKG_NAME = \"my_package\""));

        // Installs with pip (no build isolation), registers the package, and
        // discovers entry points from the installed metadata.
        assert!(script.content.contains("--no-build-isolation"));
        assert!(script.content.contains("ament_index"));
        assert!(script.content.contains("importlib.metadata"));
    }

    #[test]
    fn test_render_catkin() {
        let script =
            render_build_script("catkin", "noetic", &PathBuf::from("/pkg"), "my_package").unwrap();

        assert!(script.content.contains("/pkg"));
        assert!(script.content.contains("noetic"));
        assert_eq!(script.interpreter, None);
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
