use crate::command::Command;
use ::serde::Deserialize;
use rattler_conda_types::{Channel, NamelessMatchSpec, Platform, Version};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use serde_with::{serde_as, DisplayFromStr};
use std::collections::HashMap;

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
    #[serde(default)]
    #[serde_as(as = "HashMap<_, DisplayFromStr>")]
    pub dependencies: HashMap<String, NamelessMatchSpec>,
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
    pub platforms: Vec<Platform>,
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
