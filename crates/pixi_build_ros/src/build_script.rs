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
    render_for_platform(build_type, distro, source_dir, is_windows)
}

/// The shell reference the script uses to read `CMAKE_BUILD_TYPE` back out of
/// the environment the backend sets it in.
fn cmake_build_type_ref(is_windows: bool) -> &'static str {
    if is_windows {
        "%CMAKE_BUILD_TYPE%"
    } else {
        "${CMAKE_BUILD_TYPE}"
    }
}

/// Renders for an explicit platform. Kept separate from [`render_build_script`]
/// so the tests can cover both spellings on any machine, rather than only the
/// one they happen to run on.
fn render_for_platform(
    build_type: &str,
    distro: &str,
    source_dir: &Path,
    is_windows: bool,
) -> Result<String, BuildScriptError> {
    let template = select_template(build_type, is_windows)?;
    let src_dir_str = source_dir.display().to_string();

    // The CMake build type comes from the `cmake_build_type` variant, which
    // the backend passes to the script through the environment. Substitute a
    // reference rather than a literal so the value is resolved at build time.
    Ok(template
        .replace("@SRC_DIR@", &src_dir_str)
        .replace("@DISTRO@", distro)
        .replace("@BUILD_DIR@", "build")
        .replace("@BUILD_TYPE@", cmake_build_type_ref(is_windows)))
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
        for is_windows in [false, true] {
            let script = render_for_platform(
                "ament_cmake",
                "humble",
                &PathBuf::from("/my/source"),
                is_windows,
            )
            .unwrap();

            assert!(script.contains("/my/source"));
            // Resolved from the `cmake_build_type` variant at build time.
            assert!(script.contains(cmake_build_type_ref(is_windows)));
            assert!(!script.contains("@SRC_DIR@"));
            assert!(!script.contains("@BUILD_TYPE@"));
        }
    }

    #[test]
    fn test_cmake_build_type_is_not_hardcoded() {
        for is_windows in [false, true] {
            let script =
                render_for_platform("catkin", "noetic", &PathBuf::from("/pkg"), is_windows)
                    .unwrap();
            let reference = cmake_build_type_ref(is_windows);

            assert!(
                !script.contains("Release"),
                "a hardcoded Release survived (windows: {is_windows})"
            );
            assert!(script.contains(&format!("-DCMAKE_BUILD_TYPE={reference}")));
            assert!(script.contains(&format!("--config {reference}")));
            assert!(!script.contains("@BUILD_TYPE@"));
        }
    }

    /// Every configure and build step has to agree on the build type, or a
    /// Debug build configures Debug and then builds Release.
    #[test]
    fn test_no_template_hardcodes_a_build_type() {
        for build_type in ["ament_cmake", "catkin", "cmake"] {
            for is_windows in [false, true] {
                let script =
                    render_for_platform(build_type, "noetic", &PathBuf::from("/pkg"), is_windows)
                        .unwrap();

                for literal in ["Release", "Debug", "RelWithDebInfo"] {
                    assert!(
                        !script.contains(literal),
                        "{build_type} (windows: {is_windows}) hardcodes {literal}"
                    );
                }
            }
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
