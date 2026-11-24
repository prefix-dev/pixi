//! Build script template selection and variable substitution.

use std::path::Path;

use miette::Diagnostic;
use rattler_conda_types::Subdir;
use thiserror::Error;

use crate::distro::Distro;

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
    distro: &Distro,
    source_dir: &Path,
    pip_verbosity_flag: Option<&str>,
) -> Result<String, BuildScriptError> {
    // Use the current (build) platform, not the host/target platform.
    // The build script runs on the build machine.
    let is_windows = Subdir::current().unwrap_or(Subdir::NoArch).is_windows();
    let template = select_template(build_type, is_windows)?;

    let src_dir_str = source_dir.display().to_string();
    let rendered = template
        .replace("@SRC_DIR@", &src_dir_str)
        .replace("@DISTRO@", &distro.name)
        .replace("@PACKAGE_PREFIX@", &distro.package_prefix())
        .replace("@BUILD_DIR@", "build")
        .replace("@BUILD_TYPE@", "Release")
        .replace("@PIP_VERBOSITY@", pip_verbosity_flag.unwrap_or_default());

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
    fn test_unsupported_build_type() {
        let result = render_build_script(
            "unknown_type",
            &Distro::new("jazzy"),
            &PathBuf::from("/src"),
            None,
        );
        assert!(matches!(
            result,
            Err(BuildScriptError::UnsupportedBuildType { .. })
        ));
    }

    /// The Windows `ament_python` script installs with `setup.py` only.
    #[cfg(not(windows))]
    #[test]
    fn test_pip_verbosity_flag() {
        let pip_lines = [None, Some("-v"), Some("-vv")]
            .map(|flag| {
                let script = render_build_script(
                    "ament_python",
                    &Distro::new("jazzy"),
                    Path::new("/src"),
                    flag,
                )
                .unwrap();
                script
                    .lines()
                    .filter(|line| line.contains("pip install"))
                    .map(|line| line.trim())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .join("\n");
        insta::assert_snapshot!(pip_lines, @r"
        $PYTHON -m pip install . --no-deps --force-reinstall
        $PYTHON -m pip install . --no-deps --force-reinstall -v
        $PYTHON -m pip install . --no-deps --force-reinstall -vv
        ");
    }
}
