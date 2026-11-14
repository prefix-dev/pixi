//! Build scripts: which template builds a package, and rendering it.
//!
//! The templates are RoboStack's (they build every package of its
//! distributions), shared by this backend and RoboStack's recipe generator.
//! What differs between the two is a parameter: where the sources are
//! ([`ScriptVars::source_dir`]) and whether the build directory and host
//! prefix are reused between builds ([`BuildMode`]).

use miette::Diagnostic;
use thiserror::Error;

use crate::distro::{Distro, RosVersion};
use crate::package_xml::PackageXml;

/// Errors that can occur during build script generation.
#[derive(Debug, Error, Diagnostic)]
pub enum BuildScriptError {
    #[error("unsupported ROS build type: '{build_type}'")]
    #[diagnostic(help("Supported build types are: ament_cmake, ament_python, cmake, catkin"))]
    UnsupportedBuildType { build_type: String },
}

/// How a package is built, i.e. which template builds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildType {
    AmentCmake,
    AmentPython,
    /// A plain CMake package in ROS 2 (build type `cmake`, or `catkin`)
    Cmake,
    /// A ROS 1 package (build type `catkin` or `cmake`)
    Catkin,
}

impl BuildType {
    /// The build type of a package (with evaluated conditions).
    ///
    /// No `<build_type>` means catkin (REP 140). In ROS 2 that is a CMake
    /// package, an ament one when it builds with ament_cmake (e.g.
    /// async_web_server_cpp, mrt_cmake_modules, which only declare build types
    /// for ROS 1).
    pub fn of(xml: &PackageXml, distro: &Distro) -> Result<Self, BuildScriptError> {
        let undeclared_ament = xml.export.build_types.is_empty()
            && xml
                .dependencies
                .buildtool_depends
                .iter()
                .any(|d| d.is_active() && d.name.starts_with("ament_cmake"));
        if undeclared_ament {
            return Ok(Self::AmentCmake);
        }
        match (xml.build_type().as_str(), distro.version) {
            ("ament_cmake", _) => Ok(Self::AmentCmake),
            ("ament_python", _) => Ok(Self::AmentPython),
            ("cmake" | "catkin", RosVersion::Ros2) => Ok(Self::Cmake),
            ("cmake" | "catkin", RosVersion::Ros1) => Ok(Self::Catkin),
            (other, _) => Err(BuildScriptError::UnsupportedBuildType {
                build_type: other.to_string(),
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AmentCmake => "ament_cmake",
            Self::AmentPython => "ament_python",
            Self::Cmake => "cmake",
            Self::Catkin => "catkin",
        }
    }
}

/// Whether a build starts from scratch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    /// A fresh build directory and host prefix: a recipe built once, whose
    /// package contents are what the build added to the prefix
    Clean,
    /// The build directory and host prefix of the previous build are reused
    /// (`pixi build`): the build is incremental, and the package contents are
    /// what the build installed
    Incremental,
}

/// The values a build script template is filled with.
#[derive(Debug, Clone, Copy)]
pub struct ScriptVars<'a> {
    /// What wrote the script, for its first line (e.g. `pixi-build-ros`)
    pub generator: &'a str,
    /// The package's sources, inside double quotes in the script's shell syntax
    /// (e.g. `$SRC_DIR/$PKG_NAME/src/work/` for a recipe that checks them out;
    /// a literal path goes through [`escape_quoted`])
    pub source_dir: &'a str,
    /// The prefix of the package names, see [`Distro::package_prefix`]
    pub package_prefix: &'a str,
    /// The ROS distribution
    pub distro: &'a str,
    /// Extra CMake arguments, in the script's shell syntax
    pub cmake_args: &'a str,
}

/// The file name of the build script, e.g. `build_ament_cmake.sh`.
pub fn script_name(build_type: BuildType, windows: bool) -> &'static str {
    match (build_type, windows) {
        (BuildType::AmentCmake, _) => "build_ament_cmake.sh",
        (BuildType::AmentPython, _) => "build_ament_python.sh",
        (BuildType::Cmake, _) => "build_cmake.sh",
        (BuildType::Catkin, false) => "build_catkin.sh",
        (BuildType::Catkin, true) => "bld_catkin.bat",
    }
}

/// The interpreter the recipe runs the build script with, if not the platform's
/// default (bash, cmd.exe): the ROS 2 scripts are bash scripts, which run on
/// Windows with brush, a bash-compatible shell (in the build requirements).
pub fn script_interpreter(build_type: BuildType, windows: bool) -> Option<&'static str> {
    (windows && !is_batch(build_type, windows)).then_some("brush")
}

/// Whether the build script is a batch file rather than a bash script: only
/// ROS 1's on Windows.
pub fn is_batch(build_type: BuildType, windows: bool) -> bool {
    windows && build_type == BuildType::Catkin
}

/// Escape `text` for a double-quoted string of a build script, so that e.g. a
/// path with `$` or `%` reaches the build as it is.
pub fn escape_quoted(text: &str, batch: bool) -> String {
    if batch {
        return text.replace('%', "%%");
    }
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '$' | '`' | '"') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn template(build_type: BuildType, windows: bool) -> &'static str {
    match (build_type, windows) {
        (BuildType::AmentPython, _) => include_str!("../templates/build_ament_python.sh"),
        (BuildType::AmentCmake | BuildType::Cmake, _) => {
            include_str!("../templates/build_cmake.sh")
        }
        (BuildType::Catkin, false) => include_str!("../templates/build_catkin.sh"),
        (BuildType::Catkin, true) => include_str!("../templates/bld_catkin.bat"),
    }
}

/// Render the build script of a package (`windows` is the platform it runs on):
/// a bash script, or for ROS 1 on Windows a batch file.
///
/// Template lines starting with `@[clean]` / `@[incremental]` are only kept
/// in that [`BuildMode`]; `@(name)` is replaced by the [`ScriptVars`] field.
pub fn render_build_script(
    build_type: BuildType,
    windows: bool,
    mode: BuildMode,
    vars: &ScriptVars<'_>,
) -> String {
    let (keep, skip) = match mode {
        BuildMode::Clean => ("@[clean]", "@[incremental]"),
        BuildMode::Incremental => ("@[incremental]", "@[clean]"),
    };
    template(build_type, windows)
        .split_inclusive('\n')
        .filter(|line| !line.starts_with(skip))
        .map(|line| line.strip_prefix(keep).unwrap_or(line))
        .collect::<String>()
        .replace("@(generator)", vars.generator)
        .replace("@(source_dir)", vars.source_dir)
        .replace("@(package_prefix)", vars.package_prefix)
        .replace("@(distro)", vars.distro)
        .replace("@(cmake_args)", vars.cmake_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VARS: ScriptVars<'static> = ScriptVars {
        generator: "pixi-build-ros",
        source_dir: "/my/source",
        package_prefix: "ros2",
        distro: "jazzy",
        cmake_args: "-DFOO=ON",
    };

    fn xml(body: &str) -> PackageXml {
        PackageXml::parse(&format!(
            r#"<package format="3"><name>p</name><version>1.0.0</version>
            <description>d</description><maintainer email="a@b.c">a</maintainer>
            <license>MIT</license>{body}</package>"#
        ))
        .unwrap()
    }

    #[test]
    fn test_build_type() {
        let jazzy = Distro::new("jazzy");
        let noetic = Distro::new("noetic");
        let ament = "<export><build_type>ament_cmake</build_type></export>";
        assert_eq!(
            BuildType::of(&xml(ament), &jazzy).unwrap(),
            BuildType::AmentCmake
        );
        // no build type: catkin, i.e. CMake in ROS 2
        assert_eq!(BuildType::of(&xml(""), &jazzy).unwrap(), BuildType::Cmake);
        assert_eq!(BuildType::of(&xml(""), &noetic).unwrap(), BuildType::Catkin);
        // ... unless it builds with ament_cmake
        let undeclared = "<buildtool_depend>ament_cmake_ros</buildtool_depend>";
        assert_eq!(
            BuildType::of(&xml(undeclared), &jazzy).unwrap(),
            BuildType::AmentCmake
        );
        let unknown = "<export><build_type>meson</build_type></export>";
        assert!(matches!(
            BuildType::of(&xml(unknown), &jazzy),
            Err(BuildScriptError::UnsupportedBuildType { .. })
        ));
    }

    #[test]
    fn test_escape_quoted() {
        assert_eq!(escape_quoted(r#"C:\a $b`"%"#, false), r#"C:\\a \$b\`\"%"#);
        assert_eq!(escape_quoted(r"C:\a %b%", true), r"C:\a %%b%%");
    }

    #[test]
    fn test_every_template_is_filled() {
        for build_type in [
            BuildType::AmentCmake,
            BuildType::AmentPython,
            BuildType::Cmake,
            BuildType::Catkin,
        ] {
            for windows in [false, true] {
                for mode in [BuildMode::Clean, BuildMode::Incremental] {
                    let script = render_build_script(build_type, windows, mode, &VARS);
                    assert!(script.contains("/my/source"), "{build_type:?}");
                    assert!(!script.contains("@("), "{build_type:?} {windows}");
                    assert!(!script.contains("@["), "{build_type:?} {windows}");
                }
            }
        }
    }
}
