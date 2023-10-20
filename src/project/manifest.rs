use crate::project::python::PythonDependencies;
use crate::project::SpecType;
use crate::utils::spanned::PixiSpanned;
use crate::{consts, task::Task};
use ::serde::Deserialize;
use indexmap::IndexMap;
use miette::{Context, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use rattler_conda_types::{Channel, NamelessMatchSpec, Platform, Version};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::Deserializer;
use serde_with::de::DeserializeAsWrap;
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, PickFirst};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use url::Url;

/// Describes the contents of a project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    /// Information about the project
    pub project: ProjectMetadata,

    /// Tasks defined in the project
    #[serde(default)]
    pub tasks: HashMap<String, Task>,

    /// Additional system requirements
    #[serde(default, rename = "system-requirements")]
    pub system_requirements: SystemRequirements,

    /// The dependencies of the project.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default)]
    #[serde_as(as = "IndexMap<_, DisplayFromStr>")]
    pub dependencies: IndexMap<String, NamelessMatchSpec>,

    /// The host-dependencies of the project.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default, rename = "host-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, DisplayFromStr>>")]
    pub host_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// The build-dependencies of the project.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default, rename = "build-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, DisplayFromStr>>")]
    pub build_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// Target specific configuration.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    #[serde(default)]
    pub target: IndexMap<PixiSpanned<TargetSelector>, TargetMetadata>,

    /// Environment activation information.
    ///
    /// We use an [`IndexMap`] to preserve the order in which the items where defined in the
    /// manifest.
    pub activation: Option<Activation>,

    /// Optional python requirements
    #[serde(default, rename = "python-dependencies")]
    pub python_dependencies: PythonDependencies,
}

impl ProjectManifest {
    /// Validate the
    pub fn validate(&self, source: NamedSource, root_folder: &Path) -> miette::Result<()> {
        // Check if the targets are defined for existing platforms
        for target_sel in self.target.keys() {
            match target_sel.as_ref() {
                TargetSelector::Platform(p) => {
                    if !self.project.platforms.as_ref().contains(p) {
                        return Err(create_unsupported_platform_report(
                            source,
                            target_sel.span().unwrap_or_default(),
                            p,
                        ));
                    }
                }
            }
        }

        // parse the SPDX license expression to make sure that it is a valid expression.
        if let Some(spdx_expr) = &self.project.license {
            spdx::Expression::parse(spdx_expr)
                .into_diagnostic()
                .with_context(|| {
                    format!(
                        "failed to parse the SPDX license expression '{}'",
                        spdx_expr
                    )
                })?;
        }

        let check_file_existence = |x: &Option<PathBuf>| {
            if let Some(path) = x {
                let full_path = root_folder.join(path);
                if !full_path.exists() {
                    return Err(miette::miette!(
                        "the file '{}' does not exist",
                        full_path.display()
                    ));
                }
            }
            Ok(())
        };

        check_file_existence(&self.project.license_file)?;
        check_file_existence(&self.project.readme)?;

        Ok(())
    }

    /// Get the map of dependencies for a given spec type.
    pub fn create_or_get_dependencies(
        &mut self,
        spec_type: SpecType,
    ) -> &'_ mut IndexMap<String, NamelessMatchSpec> {
        match spec_type {
            SpecType::Run => &mut self.dependencies,
            SpecType::Host => {
                if let Some(ref mut deps) = self.host_dependencies {
                    deps
                } else {
                    self.host_dependencies.insert(IndexMap::new())
                }
            }
            SpecType::Build => {
                if let Some(ref mut deps) = self.build_dependencies {
                    deps
                } else {
                    self.build_dependencies.insert(IndexMap::new())
                }
            }
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum TargetSelector {
    // Platform specific configuration
    Platform(Platform),
    // TODO: Add minijinja coolness here.
}

struct PlatformTargetSelector;

impl<'de> DeserializeAs<'de, TargetSelector> for PlatformTargetSelector {
    fn deserialize_as<D>(deserializer: D) -> Result<TargetSelector, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(TargetSelector::Platform(Platform::deserialize(
            deserializer,
        )?))
    }
}

impl<'de> Deserialize<'de> for TargetSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            DeserializeAsWrap::<Self, PickFirst<(PlatformTargetSelector,)>>::deserialize(
                deserializer,
            )?
            .into_inner(),
        )
    }
}

#[serde_as]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetMetadata {
    /// Target specific dependencies
    #[serde(default)]
    #[serde_as(as = "IndexMap<_, DisplayFromStr>")]
    pub dependencies: IndexMap<String, NamelessMatchSpec>,

    /// The host-dependencies of the project.
    #[serde(default, rename = "host-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, DisplayFromStr>>")]
    pub host_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// The build-dependencies of the project.
    #[serde(default, rename = "build-dependencies")]
    #[serde_as(as = "Option<IndexMap<_, DisplayFromStr>>")]
    pub build_dependencies: Option<IndexMap<String, NamelessMatchSpec>>,

    /// Additional information to activate an environment.
    #[serde(default)]
    pub activation: Option<Activation>,

    /// Target specific tasks to run in the environment
    #[serde(default)]
    pub tasks: HashMap<String, Task>,
}

/// Describes the contents of the `[package]` section of the project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMetadata {
    /// The name of the project
    pub name: String,

    /// The version of the project
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,

    /// An optional project description
    pub description: Option<String>,

    /// Optional authors
    #[serde(default)]
    pub authors: Vec<String>,

    /// The channels used by the project
    #[serde_as(deserialize_as = "Vec<super::serde::ChannelStr>")]
    pub channels: Vec<Channel>,

    /// The platforms this project supports
    // TODO: This is actually slightly different from the rattler_conda_types::Platform because it
    //     should not include noarch.
    pub platforms: PixiSpanned<Vec<Platform>>,

    /// The license as a valid SPDX string (e.g. MIT AND Apache-2.0)
    pub license: Option<String>,

    /// The license file (relative to the project root)
    #[serde(rename = "license-file")]
    pub license_file: Option<PathBuf>,

    /// Path to the README file of the project (relative to the project root)
    pub readme: Option<PathBuf>,

    /// URL of the project homepage
    pub homepage: Option<Url>,

    /// URL of the project source repository
    pub repository: Option<Url>,

    /// URL of the project documentation
    pub documentation: Option<Url>,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SystemRequirements {
    pub windows: Option<bool>,

    pub unix: Option<bool>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub macos: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub linux: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    pub cuda: Option<Version>,

    pub libc: Option<LibCSystemRequirement>,

    pub archspec: Option<String>,
}

impl SystemRequirements {
    pub fn virtual_packages(&self) -> Vec<VirtualPackage> {
        let mut result = Vec::new();
        if self.windows == Some(true) {
            result.push(VirtualPackage::Win);
        }
        if self.unix == Some(true) {
            result.push(VirtualPackage::Unix);
        }
        if let Some(version) = self.linux.clone() {
            result.push(VirtualPackage::Linux(Linux { version }));
        }
        if let Some(version) = self.cuda.clone() {
            result.push(VirtualPackage::Cuda(Cuda { version }));
        }
        if let Some(version) = self.macos.clone() {
            result.push(VirtualPackage::Osx(Osx { version }))
        }
        if let Some(spec) = self.archspec.clone() {
            result.push(VirtualPackage::Archspec(Archspec { spec }))
        }
        if let Some(libc) = self.libc.clone() {
            result.push(VirtualPackage::LibC(libc.into()))
        }

        result
    }
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LibCSystemRequirement {
    /// Only a version was specified, we assume glibc.
    GlibC(#[serde_as(as = "DisplayFromStr")] Version),

    /// Specified both a family and a version.
    OtherFamily(LibCFamilyAndVersion),
}

#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibCFamilyAndVersion {
    /// The libc family, e.g. glibc
    pub family: Option<String>,

    /// The minimum version of the libc family
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,
}

impl From<LibCSystemRequirement> for LibC {
    fn from(value: LibCSystemRequirement) -> Self {
        match value {
            LibCSystemRequirement::GlibC(version) => LibC {
                version,
                family: String::from("glibc"),
            },
            LibCSystemRequirement::OtherFamily(libc) => libc.into(),
        }
    }
}

impl From<LibCFamilyAndVersion> for LibC {
    fn from(value: LibCFamilyAndVersion) -> Self {
        LibC {
            version: value.version,
            family: value.family.unwrap_or_else(|| String::from("glibc")),
        }
    }
}
#[derive(Default, Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
}

// Create an error report for using a platform that is not supported by the project.
fn create_unsupported_platform_report(
    source: NamedSource,
    span: Range<usize>,
    platform: &Platform,
) -> Report {
    miette::miette!(
        labels = vec![LabeledSpan::at(
            span,
            format!("'{}' is not a supported platform", platform)
        )],
        help = format!(
            "Add '{platform}' to the `project.platforms` array of the {} manifest.",
            consts::PROJECT_MANIFEST
        ),
        "targeting a platform that this project does not support"
    )
    .with_source_code(source)
}

#[cfg(test)]
mod test {
    use super::ProjectManifest;
    use insta::{assert_debug_snapshot, assert_display_snapshot};

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        platforms = []
        "#;

    #[test]
    fn test_target_specific() {
        let contents = format!(
            r#"
        {PROJECT_BOILERPLATE}

        [target.win-64.dependencies]
        foo = "3.4.5"

        [target.osx-64.dependencies]
        foo = "1.2.3"
        "#
        );
        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn test_dependency_types() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [dependencies]
            my-game = "1.0.0"

            [build-dependencies]
            cmake = "*"

            [host-dependencies]
            sdl2 = "*"
            "#
        );

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn test_invalid_target_specific() {
        let examples = [r#"[target.foobar.dependencies]
            invalid_platform = "henk""#];

        assert_display_snapshot!(examples
            .into_iter()
            .map(
                |example| toml_edit::de::from_str::<ProjectManifest>(&format!(
                    "{PROJECT_BOILERPLATE}\n{example}"
                ))
                .unwrap_err()
                .to_string()
            )
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_activation_scripts() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [activation]
            scripts = [".pixi/install/setup.sh"]

            [target.win-64.activation]
            scripts = [".pixi/install/setup.ps1"]

            [target.linux-64.activation]
            scripts = [".pixi/install/setup.sh", "test"]
            "#
        );

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn test_target_specific_tasks() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [tasks]
            test = "test multi"

            [target.win-64.tasks]
            test = "test win"

            [target.linux-64.tasks]
            test = "test linux"
            "#
        );

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }

    #[test]
    fn test_python_dependencies() {
        let contents = format!(
            r#"
            {PROJECT_BOILERPLATE}
            [python-dependencies]
            foo = ">=3.12"
            bar = {{ version=">=3.12", extras=["baz"] }}
            "#
        );

        assert_debug_snapshot!(
            toml_edit::de::from_str::<ProjectManifest>(&contents).expect("parsing should succeed!")
        );
    }
}
