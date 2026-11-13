//! A package's requirements, by RoboStack's rules: which package.xml
//! dependencies go to which requirement list, what every package builds with,
//! and the rules for cross-compiling and Emscripten.
//!
//! These rules build every package of RoboStack's distributions; this backend
//! and RoboStack's recipe generator share them. How a dependency becomes conda
//! specs is the caller's: this backend has no distribution index and keeps
//! package.xml's versions, RoboStack checks its distribution.

use indexmap::IndexSet;
use rattler_conda_types::Subdir;
use serde::Serialize;

use crate::build_script::BuildType;
use crate::distro::{Distro, RosVersion};
use crate::package_xml::{Dependency, PackageXml};

/// A requirement: a match spec, or one only used when cross-compiling (or only
/// when not). The other platform conditions are resolved when the requirements
/// are made, for one target platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub enum Req {
    Spec(String),
    If {
        #[serde(rename = "if")]
        cond: BuildPlatform,
        then: Vec<String>,
    },
}

/// Build vs target platform; `Ord` as the conditions' text sorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum BuildPlatform {
    #[serde(rename = "build_platform != target_platform")]
    Cross,
    #[serde(rename = "build_platform == target_platform")]
    Native,
}

impl BuildPlatform {
    /// The condition, as a recipe writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cross => "build_platform != target_platform",
            Self::Native => "build_platform == target_platform",
        }
    }
}

impl Req {
    pub fn cross(s: impl Into<String>) -> Self {
        Req::If {
            cond: BuildPlatform::Cross,
            then: vec![s.into()],
        }
    }
    pub fn native(s: impl Into<String>) -> Self {
        Req::If {
            cond: BuildPlatform::Native,
            then: vec![s.into()],
        }
    }
    fn sort_key(&self) -> (Option<BuildPlatform>, String) {
        match self {
            Req::Spec(s) => (None, s.clone()),
            Req::If { cond, then } => (Some(*cond), then.join(",")),
        }
    }
}

/// An ordered, duplicate-free requirement list.
#[derive(Debug, Clone, Default)]
pub struct Reqs(IndexSet<Req>);

impl Reqs {
    pub fn push(&mut self, r: Req) {
        self.0.insert(r);
    }
    pub fn spec(&mut self, s: impl Into<String>) {
        self.push(Req::Spec(s.into()));
    }
    pub fn extend(&mut self, specs: impl IntoIterator<Item = impl Into<String>>) {
        for s in specs {
            self.spec(s);
        }
    }
    pub fn contains(&self, s: &str) -> bool {
        self.0.contains(&Req::Spec(s.to_string()))
    }
    pub fn iter(&self) -> impl Iterator<Item = &Req> {
        self.0.iter()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Remove the requirements on package `name` (a spec's first word), also
    /// inside conditional requirements; the removed specs.
    pub fn remove(&mut self, name: &str) -> Vec<String> {
        let is = |s: &String| s.split_whitespace().next() == Some(name);
        let mut removed = vec![];
        self.0 = std::mem::take(&mut self.0)
            .into_iter()
            .filter_map(|r| match r {
                Req::Spec(s) if is(&s) => {
                    removed.push(s);
                    None
                }
                Req::If { cond, then } => {
                    let (gone, keep): (Vec<String>, Vec<String>) = then.into_iter().partition(is);
                    removed.extend(gone);
                    (!keep.is_empty()).then_some(Req::If { cond, then: keep })
                }
                r => Some(r),
            })
            .collect();
        removed
    }
    /// Replace a spec, also inside conditional requirements.
    pub fn replace(&mut self, from: &str, to: &str) {
        let swap = |s: &String| {
            if s == from { to.to_string() } else { s.clone() }
        };
        self.0 = std::mem::take(&mut self.0)
            .into_iter()
            .map(|r| match r {
                Req::Spec(s) => Req::Spec(swap(&s)),
                Req::If { cond, then } => Req::If {
                    cond,
                    then: then.iter().map(swap).collect(),
                },
            })
            .collect();
    }
    fn sort(&mut self) {
        self.0.sort_by_key(Req::sort_key);
    }
}

impl Serialize for Reqs {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter())
    }
}

/// A package's build, host and run requirements.
#[derive(Debug, Clone, Default)]
pub struct Requirements {
    pub build: Reqs,
    pub host: Reqs,
    pub run: Reqs,
}

/// What a dependency is resolved for: run dependencies may drop versions
/// (run_exports pin them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Build,
    Run,
}

/// Python for a package built without it (an interpreter for ament's scripts),
/// pinned so that it isn't a variant.
pub const BUILD_TIME_PYTHON: &str = "python ${{ python_min | default('3.11') }}.*";

/// Dependencies that mean a package's own artifact needs Python
/// (rosidl_default_generators doesn't: rclcpp only uses it for test messages;
/// message packages are found by their group instead).
const PYTHON_MARKERS: [&str; 5] = [
    "rclpy",
    "pybind11",
    "python_cmake_module",
    "ament_cmake_python",
    "rosidl_pycommon",
];

/// The ROS packages that every ROS 2 package builds on, and that themselves
/// don't.
pub const CORE: [&str; 4] = [
    "ament_cmake_core",
    "ament_package",
    "ros_workspace",
    "ros_environment",
];

fn active<'a>(lists: &[&'a Vec<Dependency>]) -> Vec<&'a Dependency> {
    lists
        .iter()
        .flat_map(|l| l.iter())
        .filter(|d| d.is_active())
        .collect()
}

/// Whether a package's own artifact needs Python (host/run), i.e. is built per
/// Python version. Conservative: a false "yes" only costs builds.
pub fn python_needed(xml: &PackageXml, build_type: BuildType) -> bool {
    if build_type == BuildType::AmentPython {
        return true;
    }
    if xml
        .member_of_groups
        .iter()
        .any(|g| g.is_active() && g.name == "rosidl_interface_packages")
    {
        return true;
    }
    let d = &xml.dependencies;
    let deps = active(&[
        &d.build_depends,
        &d.build_export_depends,
        &d.exec_depends,
        &d.run_depends,
        &d.test_depends,
        &d.depend,
    ]);
    let tools = active(&[&d.buildtool_depends, &d.buildtool_export_depends]);
    // build tools only by the markers: a plain python3 buildtool_depend is an
    // interpreter for a code generator (BUILD_TIME_PYTHON)
    deps.iter()
        .chain(&tools)
        .any(|d| PYTHON_MARKERS.contains(&d.name.as_str()))
        || deps
            .iter()
            .any(|d| d.name.contains("python") || d.name.starts_with("pybind"))
}

/// What the requirements are made for.
#[derive(Debug, Clone)]
pub struct Rules {
    pub distro: Distro,
    /// The prefix of the ROS packages' conda names (default:
    /// [`Distro::package_prefix`])
    pub package_prefix: String,
    /// The target platform
    pub platform: Subdir,
    /// The distro mutex spec, in host and run (default: [`Distro::mutex_spec`])
    pub mutex: Option<String>,
    /// Whether the variant config has `c_stdlib`, i.e. `stdlib('c')` renders
    pub c_stdlib: bool,
    /// Build a package without Python unless it needs it ([`python_needed`]),
    /// once instead of per Python version
    pub conditional_python: bool,
}

impl Rules {
    pub fn new(distro: Distro, platform: Subdir) -> Self {
        Self {
            package_prefix: distro.package_prefix().to_string(),
            mutex: Some(distro.mutex_spec()),
            distro,
            platform,
            c_stdlib: true,
            conditional_python: false,
        }
    }

    /// The conda name of a ROS package.
    pub fn conda_name(&self, ros: &str) -> String {
        format!("{}-{}", self.package_prefix, ros.replace('_', "-"))
    }

    fn emscripten(&self) -> bool {
        self.platform.only_platform() == Some("emscripten")
    }

    /// The requirements of a package before [`Self::finish`]: what every
    /// package builds with and its package.xml dependencies, resolved by
    /// `resolve`. `groups` are the ROS packages its `<group_depend>`s stand for.
    pub fn requirements<E>(
        &self,
        xml: &PackageXml,
        build_type: BuildType,
        groups: &[String],
        mut resolve: impl FnMut(&Dependency, DepKind) -> Result<Vec<String>, E>,
    ) -> Result<Requirements, E> {
        let unix = !self.platform.is_windows();
        let osx = self.platform.is_osx();
        let needs_python = !self.conditional_python || python_needed(xml, build_type);

        let mut build = Reqs::default();
        build.extend(["${{ compiler('cxx') }}", "${{ compiler('c') }}"]);
        if self.c_stdlib && !self.emscripten() {
            build.spec("${{ stdlib('c') }}");
        }
        build.extend(["ninja", "python", "setuptools", "git", "git-lfs"]);
        if unix {
            build.extend(["patch", "make", "coreutils"]);
        } else {
            build.spec("m2-patch");
        }
        if osx {
            build.spec("tapi");
        }
        build.extend(["cmake", "cython"]);
        for s in [
            "pkg-config",
            "python",
            "cross-python_${{ target_platform }}",
            "numpy",
        ] {
            build.push(Req::cross(s));
        }
        if osx && build_type != BuildType::AmentPython {
            build.spec(
                "clang-tools ${{ (cxx_compiler_version ~ '.*') if cxx_compiler_version is defined else '*' }}",
            );
        }
        let mut host = Reqs::default();
        host.push(Req::native("pkg-config"));
        let mut run = Reqs::default();
        if needs_python {
            host.extend(["python", "numpy", "pip"]);
            let python = resolve(&Dependency::from("python"), DepKind::Build)?;
            host.extend(python.clone());
            run.extend(python);
        }
        if build_type == BuildType::AmentPython {
            host.extend(resolve(
                &Dependency::from("python-setuptools"),
                DepKind::Build,
            )?);
        }
        if let Some(m) = &self.mutex {
            host.spec(m);
            run.spec(m);
        }
        if self.distro.version == RosVersion::Ros2 && !CORE.contains(&xml.name.as_str()) {
            host.extend([
                self.conda_name("ros_environment"),
                self.conda_name("ros_workspace"),
            ]);
            run.spec(self.conda_name("ros_workspace"));
        }

        let groups: Vec<Dependency> = groups
            .iter()
            .map(|g| Dependency::from(g.as_str()))
            .collect();
        let d = &xml.dependencies;
        let mut host_deps = active(&[
            &d.build_depends,
            &d.depend,
            &d.build_export_depends,
            &d.test_depends,
        ]);
        // a format 1 run_depend is also a build dependency
        if xml.format == 1 {
            host_deps.extend(active(&[&d.run_depends]));
        }
        host_deps.extend(&groups);
        // build tools go to host, except git, which has to run on the build machine
        for tool in active(&[&d.buildtool_depends, &d.buildtool_export_depends]) {
            let resolved = resolve(tool, DepKind::Build)?;
            if resolved.iter().any(|r| r == "git") {
                build.extend(resolved);
            } else if self.conditional_python && resolved == ["python"] {
                // an interpreter for a code generator: BUILD_TIME_PYTHON
            } else if !resolved.is_empty() && tool.name != "cmake" {
                host_deps.push(tool);
            }
        }
        // cyclonedds generates code during the build
        if xml.name == "cyclonedds" || host_deps.iter().any(|d| d.name == "cyclonedds") {
            build.push(Req::cross(self.conda_name("cyclonedds")));
        }
        for dep in host_deps {
            host.extend(resolve(dep, DepKind::Build)?);
        }
        let mut run_deps = active(&[
            &d.run_depends,
            &d.exec_depends,
            &d.depend,
            &d.build_export_depends,
            &d.buildtool_export_depends,
        ]);
        run_deps.extend(&groups);
        for dep in run_deps {
            run.extend(resolve(dep, DepKind::Run)?);
        }
        Ok(Requirements { build, host, run })
    }

    /// The rules that apply to the complete requirements (after RoboStack's
    /// per-package changes, which can e.g. give a package Python): build-time
    /// Python for a package without Python, the requirements that break when
    /// cross-compiling or building for Emscripten, and host and run sorted.
    /// Whether the package is built without Python.
    pub fn finish(&self, reqs: &mut Requirements) -> bool {
        let Requirements { build, host, run } = reqs;
        let python_free = self.conditional_python && !host.contains("python");
        if python_free {
            build.replace("python", BUILD_TIME_PYTHON);
        }

        let emscripten = self.emscripten();
        // cmake is a build tool, never a host or (on Emscripten) run dependency
        for spec in host.remove("cmake") {
            build.spec(spec);
        }
        if emscripten {
            run.remove("cmake");
        }
        // spellchecker:off
        // mimick_vendor runs on the build machine; it doesn't build for Emscripten
        let vendor = self.conda_name("mimick_vendor");
        // spellchecker:on
        let had = host.remove(&vendor);
        if emscripten {
            build.remove(&vendor);
        } else {
            build.extend(had);
        }
        // Emscripten resolves the generators out of `build`
        let generators = self.conda_name("rosidl_default_generators");
        if emscripten && host.contains(&generators) {
            build.spec(generators);
        }
        if host.contains(&self.conda_name("pybind11_vendor")) {
            host.spec("pybind11");
        }
        for d in ["pybind11", "qt-main"] {
            if host.contains(d) {
                build.push(Req::cross(d));
            }
        }
        // build-machine tools: host when native, build when cross-compiling
        for d in ["pyqt-builder", "git", "doxygen", "git-lfs"] {
            for spec in host.remove(d) {
                host.push(Req::native(spec.clone()));
                build.push(Req::cross(spec));
            }
        }
        host.sort();
        run.sort();
        python_free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reqs(r: Vec<Req>) -> Reqs {
        let mut out = Reqs::default();
        r.into_iter().for_each(|r| out.push(r));
        out
    }

    #[test]
    fn remove_by_package_name() {
        let mut r = reqs(vec![
            Req::Spec("foo >=1".into()),
            Req::Spec("foo-bar".into()),
            Req::native("foo"),
            Req::If {
                cond: BuildPlatform::Cross,
                then: vec!["foo 2.*".into(), "baz".into()],
            },
        ]);
        assert_eq!(r.remove("foo"), ["foo >=1", "foo", "foo 2.*"]);
        assert_eq!(
            r.0.into_iter().collect::<Vec<_>>(),
            [Req::Spec("foo-bar".into()), Req::cross("baz")]
        );
    }

    fn xml(body: &str) -> PackageXml {
        PackageXml::parse(&format!(
            r#"<package format="3"><name>demo</name><version>1.0.0</version>
            <description>d</description><maintainer email="a@b.c">a</maintainer>
            <license>MIT</license>{body}</package>"#
        ))
        .unwrap()
    }

    /// ROS packages by name, `python3` to python.
    fn resolve(dep: &Dependency, _: DepKind) -> Result<Vec<String>, ()> {
        Ok(match dep.name.as_str() {
            "python" | "python3" => vec!["python".into()],
            "python-setuptools" => vec!["setuptools".into()],
            "cmake" => vec!["cmake".into()],
            name => vec![format!("ros2-{}", name.replace('_', "-"))],
        })
    }

    fn render(rules: &Rules, body: &str, build_type: BuildType) -> String {
        let mut reqs = rules
            .requirements(&xml(body), build_type, &[], resolve)
            .unwrap();
        rules.finish(&mut reqs);
        serde_yaml::to_string(&serde_json::json!({
            "build": serde_json::to_value(&reqs.build).unwrap(),
            "host": serde_json::to_value(&reqs.host).unwrap(),
            "run": serde_json::to_value(&reqs.run).unwrap(),
        }))
        .unwrap()
    }

    // spellchecker:off
    #[test]
    fn test_requirements_cross_compiled_osx() {
        let rules = Rules::new(Distro::new("jazzy"), Subdir::OsxArm64);
        let body = "<buildtool_depend>ament_cmake</buildtool_depend>
            <buildtool_depend>cmake</buildtool_depend>
            <depend>rclcpp</depend><depend>mimick_vendor</depend>
            <exec_depend>python3</exec_depend>";
        insta::assert_snapshot!(render(&rules, body, BuildType::AmentCmake), @"
        build:
        - ${{ compiler('cxx') }}
        - ${{ compiler('c') }}
        - ${{ stdlib('c') }}
        - ninja
        - python
        - setuptools
        - git
        - git-lfs
        - patch
        - make
        - coreutils
        - tapi
        - cmake
        - cython
        - if: build_platform != target_platform
          then:
          - pkg-config
        - if: build_platform != target_platform
          then:
          - python
        - if: build_platform != target_platform
          then:
          - cross-python_${{ target_platform }}
        - if: build_platform != target_platform
          then:
          - numpy
        - clang-tools ${{ (cxx_compiler_version ~ '.*') if cxx_compiler_version is defined else '*' }}
        - ros2-mimick-vendor
        host:
        - numpy
        - pip
        - python
        - ros2-ament-cmake
        - ros2-distro-mutex * jazzy_*
        - ros2-rclcpp
        - ros2-ros-environment
        - ros2-ros-workspace
        - if: build_platform == target_platform
          then:
          - pkg-config
        run:
        - python
        - ros2-distro-mutex * jazzy_*
        - ros2-mimick-vendor
        - ros2-rclcpp
        - ros2-ros-workspace
        ");
    }
    // spellchecker:on

    /// Conditional Python: a C++ package with a code generator is built once,
    /// with an interpreter only in build.
    #[test]
    fn test_requirements_without_python() {
        let rules = Rules {
            conditional_python: true,
            ..Rules::new(Distro::new("lyrical"), Subdir::Linux64)
        };
        let body = "<buildtool_depend>ament_cmake</buildtool_depend>
            <buildtool_depend>python3</buildtool_depend>
            <depend>rclcpp</depend>";
        let mut reqs = rules
            .requirements(&xml(body), BuildType::AmentCmake, &[], resolve)
            .unwrap();
        assert!(rules.finish(&mut reqs));
        assert!(reqs.build.contains(BUILD_TIME_PYTHON));
        assert!(!reqs.host.contains("python"));
        assert!(!reqs.run.contains("python"));

        let python = "<depend>rclpy</depend>";
        assert!(python_needed(&xml(python), BuildType::AmentCmake));
        assert!(python_needed(&xml(""), BuildType::AmentPython));
        assert!(!python_needed(&xml(body), BuildType::AmentCmake));
    }
}
