use rattler_conda_types::Version;
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use std::str::FromStr;

/// Describes the minimal system requirements to be able to run a certain environment.
#[serde_as]
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SystemRequirements {
    /// Dictates the presence of the __win virtual package.
    pub windows: Option<bool>,

    /// Dictates the presence of the __unix virtual package.
    pub unix: Option<bool>,

    /// Dictates the minimum version of macOS required.
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub macos: Option<Version>,

    /// Dictates the minimum linux version required.
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub linux: Option<Version>,

    /// Dictates the minimum cuda version required.
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub cuda: Option<Version>,

    /// Dictates information about the libc version (and optional family).
    pub libc: Option<LibCSystemRequirement>,

    /// Information about the system architecture.
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

#[derive(Debug, Clone)]
pub enum LibCSystemRequirement {
    /// Only a version was specified, we assume glibc.
    GlibC(Version),

    /// Specified both a family and a version.
    OtherFamily(LibCFamilyAndVersion),
}

impl<'de> Deserialize<'de> for LibCSystemRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(LibCSystemRequirement::OtherFamily))
            .string(|s| {
                Version::from_str(s)
                    .map(LibCSystemRequirement::GlibC)
                    .map_err(serde::de::Error::custom)
            })
            .expecting("a version or a mapping with `family` and `version`")
            .deserialize(deserializer)
    }
}

impl LibCSystemRequirement {
    /// Returns the family and version of this libc requirement.
    pub fn family_and_version(&self) -> (&str, &Version) {
        match self {
            LibCSystemRequirement::GlibC(version) => ("glibc", version),
            LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion { family, version: v }) => {
                (family.as_deref().unwrap_or("glibc"), v)
            }
        }
    }
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

#[cfg(test)]
mod test {
    use super::*;
    use insta::assert_snapshot;
    use rattler_conda_types::Version;
    use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackage};
    use serde::Deserialize;
    use std::str::FromStr;

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
        windows = true
        unix = true
        linux = "5.11"
        cuda = "12.2"
        macos = "10.15"
        archspec = "arm64"
        libc = { family = "glibc", version = "2.12" }
        "#;

        let system_requirements: SystemRequirements =
            toml_edit::de::from_str(file_content).unwrap();

        let expected_requirements: Vec<VirtualPackage> = vec![
            VirtualPackage::Win,
            VirtualPackage::Unix,
            VirtualPackage::Linux(Linux {
                version: Version::from_str("5.11").unwrap(),
            }),
            VirtualPackage::Cuda(Cuda {
                version: Version::from_str("12.2").unwrap(),
            }),
            VirtualPackage::Osx(Osx {
                version: Version::from_str("10.15").unwrap(),
            }),
            VirtualPackage::Archspec(Archspec {
                spec: "arm64".to_string(),
            }),
            VirtualPackage::LibC(LibC {
                version: Version::from_str("2.12").unwrap(),
                family: "glibc".to_string(),
            }),
        ];

        assert_eq!(
            system_requirements.virtual_packages(),
            expected_requirements
        );
    }

    #[test]
    fn test_system_requirements_failing_edge_cases() {
        #[derive(Deserialize)]
        struct Manifest {
            #[serde(rename = "system-requirements")]
            _system_requirements: SystemRequirements,
        }

        let file_contents = [
            (
                "version_misspelled",
                r#"
        [system-requirements]
        libc = { verion = "2.12" }
        "#,
            ),
            (
                "unknown_key",
                r#"
        [system-requirements]
        lib = "2.12"
        "#,
            ),
            (
                "fam_misspelled",
                r#"
        [system-requirements.libc]
        version = "2.12"
        fam = "glibc"
        "#,
            ),
            (
                "lic_misspelled",
                r#"
        [system-requirements.lic]
        version = "2.12"
        family = "glibc"
        "#,
            ),
        ];

        for (name, file_content) in file_contents {
            let error = match toml_edit::de::from_str::<Manifest>(file_content) {
                Ok(_) => panic!("Expected error"),
                Err(e) => e.to_string(),
            };
            assert_snapshot!(name, &error, file_content);
        }
    }
}
