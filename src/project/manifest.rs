use crate::command::Command;
use crate::consts::PROJECT_MANIFEST;
use crate::report_error::ReportError;
use ::serde::Deserialize;
use ariadne::{ColorGenerator, Fmt, Label, Report, ReportKind, Source};
use indexmap::IndexMap;
use rattler_conda_types::{Channel, NamelessMatchSpec, Platform, Version};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::Deserializer;
use serde_spanned::Spanned;
use serde_with::de::DeserializeAsWrap;
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, PickFirst};
use std::collections::HashMap;
use std::ops::Range;

/// Describes the contents of a project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectManifest {
    /// Information about the project
    pub project: ProjectMetadata,

    /// Commands defined in the project
    #[serde(default)]
    pub commands: HashMap<String, Command>,

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
    pub target: IndexMap<Spanned<TargetSelector>, TargetMetadata>,
}

impl ProjectManifest {
    /// Validate the
    pub fn validate(&self, contents: &str) -> anyhow::Result<()> {
        // Check if the targets are defined for existing platforms
        for target_sel in self.target.keys() {
            match target_sel.as_ref() {
                TargetSelector::Platform(p) => {
                    if !self.project.platforms.as_ref().contains(p) {
                        return Err(create_unsupported_platform_report(
                            contents,
                            target_sel.span(),
                            p,
                        )
                        .into());
                    }
                }
            }
        }

        Ok(())
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
#[derive(Debug, Clone, Deserialize)]
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
}

/// Describes the contents of the `[package]` section of the project manifest.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
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
    pub platforms: Spanned<Vec<Platform>>,
}

#[serde_as]
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SystemRequirements {
    windows: Option<bool>,

    unix: Option<bool>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    macos: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    linux: Option<Version>,

    #[serde_as(as = "Option<DisplayFromStr>")]
    cuda: Option<Version>,

    libc: Option<LibCSystemRequirement>,

    archspec: Option<String>,
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

// Create an error report for usign a platform that is not supported by the project.
fn create_unsupported_platform_report(
    source: &str,
    span: Range<usize>,
    p: &Platform,
) -> ReportError {
    let mut color_generator = ColorGenerator::new();
    let platform = color_generator.next();

    let report = Report::build(ReportKind::Error, PROJECT_MANIFEST, span.start)
        .with_message("Targeting a platform that this project does not support")
        .with_label(
            Label::new((PROJECT_MANIFEST, span))
                .with_message(format!("'{}' is not a supported platform", p.fg(platform)))
                .with_color(platform),
        )
        .with_help(format!(
            "Add '{}' to the `project.platforms` array of the {PROJECT_MANIFEST} manifest.",
            p.fg(platform)
        ))
        .finish();

    ReportError {
        report,
        source: (PROJECT_MANIFEST, Source::from(source)),
    }
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
}
