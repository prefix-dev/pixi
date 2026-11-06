//! Build script template selection and variable substitution.

use std::path::Path;

use miette::Diagnostic;
use rattler_build_recipe::stage0::{
    Build, Conditional, Item, JinjaExpression, JinjaTemplate, NestedItemList, Value,
};
use rattler_conda_types::Subdir;
use thiserror::Error;

/// Variant key that selects the `CMAKE_BUILD_TYPE` of CMake based packages.
pub const CMAKE_BUILD_TYPE_KEY: &str = "cmake_build_type";

/// Environment variable the build script templates read the CMake build type
/// from.
pub const CMAKE_BUILD_TYPE_ENV: &str = "ROS_CMAKE_BUILD_TYPE";

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
        .replace("@BUILD_DIR@", "build");

    Ok(rendered)
}

/// Whether packages of this ROS build type are built with CMake, and can
/// therefore be built as debug variants.
pub fn uses_cmake(build_type: &str) -> bool {
    matches!(build_type, "ament_cmake" | "cmake" | "catkin")
}

/// Turn the CMake build type into a variant of the package.
///
/// Every build type other than `Release` gets a flag named after it
/// (`debug`, `relwithdebinfo`, ...), so it can be requested with e.g.
/// `flags = ["debug"]`. Those variants are down-prioritized so the solver
/// keeps picking the release build when no flag is requested.
///
/// Returns the script environment value that passes the build type to the
/// build script.
pub fn add_cmake_build_type_variant(build: &mut Build) -> Value<String> {
    build.flags.push(Item::Conditional(Conditional {
        condition: JinjaExpression::new(format!("{CMAKE_BUILD_TYPE_KEY} != 'Release'"))
            .expect("valid jinja expression"),
        then: NestedItemList::single(Item::Value(Value::new_template(
            JinjaTemplate::new(format!("${{{{ {CMAKE_BUILD_TYPE_KEY} | lower }}}}"))
                .expect("valid jinja template"),
            None,
        ))),
        else_value: None,
        condition_span: None,
    }));

    build.variant.down_prioritize_variant = Some(Value::new_template(
        JinjaTemplate::new(format!(
            "${{{{ 0 if {CMAKE_BUILD_TYPE_KEY} == 'Release' else 1 }}}}"
        ))
        .expect("valid jinja template"),
        None,
    ));

    Value::new_template(
        JinjaTemplate::new(format!("${{{{ {CMAKE_BUILD_TYPE_KEY} }}}}"))
            .expect("valid jinja template"),
        None,
    )
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
        assert!(script.contains("ROS_CMAKE_BUILD_TYPE"));
        assert!(!script.contains("@SRC_DIR@"));
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
