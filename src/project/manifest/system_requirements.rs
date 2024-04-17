use miette::Diagnostic;
use rattler_conda_types::Version;
use rattler_virtual_packages::{Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use std::str::FromStr;
use thiserror::Error;

const GLIBC_FAMILY: &str = "glibc";

/// Describes the minimal system requirements to be able to run a certain environment.
#[serde_as]
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SystemRequirements {
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
        if let Some(version) = self.linux.clone() {
            result.push(VirtualPackage::Linux(Linux { version }));
        }
        if let Some(version) = self.cuda.clone() {
            result.push(VirtualPackage::Cuda(Cuda { version }));
        }
        if let Some(version) = self.macos.clone() {
            result.push(VirtualPackage::Osx(Osx { version }))
        }
        if let Some(libc) = self.libc.clone() {
            result.push(VirtualPackage::LibC(libc.into()))
        }
        if let Some(_archspec) = self.archspec.clone() {
            tracing::info!("The archspec system-requirement is deprecated and not used.");
        }
        result
    }

    /// Returns the combination of two system requirements.
    ///
    /// If both system requirements specify the same virtual package, the highest version is taken.
    ///
    /// An error if returned if two specs cannot be combined.
    pub fn union(&self, other: &Self) -> Result<Self, SystemRequirementsUnionError> {
        let linux = match (&self.linux, &other.linux) {
            (Some(linux), Some(other_linux)) => Some(linux.max(other_linux).clone()),
            (None, Some(other_linux)) => Some(other_linux.clone()),
            (linux, _) => linux.clone(),
        };

        let cuda = match (&self.cuda, &other.cuda) {
            (Some(cuda), Some(other_cuda)) => Some(cuda.max(other_cuda).clone()),
            (None, Some(other_cuda)) => Some(other_cuda.clone()),
            (cuda, _) => cuda.clone(),
        };

        let macos = match (&self.macos, &other.macos) {
            (Some(macos), Some(other_macos)) => Some(macos.max(other_macos).clone()),
            (None, Some(other_macos)) => Some(other_macos.clone()),
            (macos, _) => macos.clone(),
        };

        let libc = match (&self.libc, &other.libc) {
            (Some(libc), Some(other_libc)) => {
                let (family_a, version_a) = libc.family_and_version();
                let (family_b, version_b) = other_libc.family_and_version();
                if family_a != family_b {
                    return Err(SystemRequirementsUnionError::DifferentLibcFamilies(
                        family_a.to_string(),
                        family_b.to_string(),
                    ));
                }

                let version = version_a.max(version_b).clone();
                if family_a == GLIBC_FAMILY {
                    Some(LibCSystemRequirement::GlibC(version))
                } else {
                    Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                        family: Some(family_a.to_string()),
                        version,
                    }))
                }
            }
            (None, Some(other_libc)) => Some(other_libc.clone()),
            (libc, _) => libc.clone(),
        };

        let archspec = match (&self.archspec, &other.archspec) {
            (Some(archspec), Some(other_archspec)) => {
                if archspec != other_archspec {
                    return Err(SystemRequirementsUnionError::MismatchingArchSpec(
                        archspec.to_string(),
                        other_archspec.to_string(),
                    ));
                }
                Some(archspec.clone())
            }
            (None, Some(other_archspec)) => Some(other_archspec.clone()),
            (archspec, _) => archspec.clone(),
        };

        Ok(Self {
            linux,
            cuda,
            macos,
            libc,
            archspec,
        })
    }
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SystemRequirementsUnionError {
    #[error("two different libc families were specified: '{0}' and '{1}'")]
    DifferentLibcFamilies(String, String),

    #[error("cannot combine archspecs: '{0}' and '{1}'")]
    MismatchingArchSpec(String, String),
}

#[derive(Debug, Clone)]
pub enum LibCSystemRequirement {
    /// Only a version was specified, we assume glibc.
    GlibC(Version),

    /// Specified both a family and a version.
    OtherFamily(LibCFamilyAndVersion),
}

impl PartialEq for LibCSystemRequirement {
    fn eq(&self, other: &Self) -> bool {
        let (family_a, version_a) = self.family_and_version();
        let (family_b, version_b) = other.family_and_version();
        family_a == family_b && version_a == version_b
    }
}

impl Eq for LibCSystemRequirement {}

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
            LibCSystemRequirement::GlibC(version) => (GLIBC_FAMILY, version),
            LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion { family, version: v }) => {
                (family.as_deref().unwrap_or(GLIBC_FAMILY), v)
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
                family: String::from(GLIBC_FAMILY),
            },
            LibCSystemRequirement::OtherFamily(libc) => libc.into(),
        }
    }
}

impl From<LibCFamilyAndVersion> for LibC {
    fn from(value: LibCFamilyAndVersion) -> Self {
        LibC {
            version: value.version,
            family: value.family.unwrap_or_else(|| String::from(GLIBC_FAMILY)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use rattler_conda_types::Version;
    use rattler_virtual_packages::{Cuda, LibC, Linux, Osx, VirtualPackage};
    use serde::Deserialize;
    use std::str::FromStr;

    #[test]
    fn system_requirements_works() {
        let file_content = r#"
        linux = "5.11"
        cuda = "12.2"
        macos = "10.15"
        libc = { family = "glibc", version = "2.12" }
        "#;

        let system_requirements: SystemRequirements =
            toml_edit::de::from_str(file_content).unwrap();

        let expected_requirements: Vec<VirtualPackage> = vec![
            VirtualPackage::Linux(Linux {
                version: Version::from_str("5.11").unwrap(),
            }),
            VirtualPackage::Cuda(Cuda {
                version: Version::from_str("12.2").unwrap(),
            }),
            VirtualPackage::Osx(Osx {
                version: Version::from_str("10.15").unwrap(),
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

    #[test]
    fn test_empty_union() {
        assert_eq!(
            SystemRequirements::default()
                .union(&SystemRequirements::default())
                .unwrap(),
            SystemRequirements::default()
        );
    }

    #[test]
    fn test_union_cuda() {
        let a = SystemRequirements {
            cuda: Some(Version::from_str("11.0").unwrap()),
            ..Default::default()
        };
        let b = SystemRequirements {
            cuda: Some(Version::from_str("12.0").unwrap()),
            ..Default::default()
        };
        let c = SystemRequirements {
            cuda: None,
            ..Default::default()
        };
        assert_eq!(
            a.union(&b).unwrap(),
            SystemRequirements {
                cuda: Some(Version::from_str("12.0").unwrap()),
                ..Default::default()
            }
        );
        assert_eq!(b.union(&a).unwrap(), a.union(&b).unwrap());
        assert_eq!(
            c.union(&b).unwrap(),
            SystemRequirements {
                cuda: Some(Version::from_str("12.0").unwrap()),
                ..Default::default()
            }
        );
        assert_eq!(c.union(&b).unwrap(), b.union(&c).unwrap());
    }

    #[test]
    fn test_union_linux() {
        let a = SystemRequirements {
            linux: Some(Version::from_str("5.3.0").unwrap()),
            ..Default::default()
        };
        let b = SystemRequirements {
            linux: Some(Version::from_str("4.2.12").unwrap()),
            ..Default::default()
        };
        let c = SystemRequirements {
            linux: None,
            ..Default::default()
        };
        assert_eq!(
            a.union(&b).unwrap(),
            SystemRequirements {
                linux: Some(Version::from_str("5.3.0").unwrap()),
                ..Default::default()
            }
        );
        assert_eq!(b.union(&a).unwrap(), a.union(&b).unwrap());
        assert_eq!(
            c.union(&b).unwrap(),
            SystemRequirements {
                linux: Some(Version::from_str("4.2.12").unwrap()),
                ..Default::default()
            }
        );
        assert_eq!(c.union(&b).unwrap(), b.union(&c).unwrap());
    }

    #[test]
    fn test_union_macos() {
        let a = SystemRequirements {
            macos: Some(Version::from_str("13.6.2").unwrap()),
            ..Default::default()
        };
        let b = SystemRequirements {
            macos: Some(Version::from_str("12.7").unwrap()),
            ..Default::default()
        };
        let c = SystemRequirements {
            macos: None,
            ..Default::default()
        };
        assert_eq!(
            a.union(&b).unwrap(),
            SystemRequirements {
                macos: Some(Version::from_str("13.6.2").unwrap()),
                ..Default::default()
            }
        );
        assert_eq!(b.union(&a).unwrap(), a.union(&b).unwrap());
        assert_eq!(
            c.union(&b).unwrap(),
            SystemRequirements {
                macos: Some(Version::from_str("12.7").unwrap()),
                ..Default::default()
            }
        );
        assert_eq!(c.union(&b).unwrap(), b.union(&c).unwrap());
    }

    #[test]
    fn test_union_libc() {
        let glibc_2_12 = SystemRequirements {
            libc: Some(LibCSystemRequirement::GlibC(
                Version::from_str("2.12").unwrap(),
            )),
            ..Default::default()
        };
        let glibc_fam_2_17 = SystemRequirements {
            libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: Some(String::from(GLIBC_FAMILY)),
                version: Version::from_str("2.17").unwrap(),
            })),
            ..Default::default()
        };
        let libc_def_fam_2_19 = SystemRequirements {
            libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: None,
                version: Version::from_str("2.19").unwrap(),
            })),
            ..Default::default()
        };
        let eglibc_2_17 = SystemRequirements {
            libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: Some(String::from("eglibc")),
                version: Version::from_str("2.17").unwrap(),
            })),
            ..Default::default()
        };
        assert_eq!(
            glibc_2_12.union(&glibc_fam_2_17).unwrap(),
            SystemRequirements {
                libc: Some(LibCSystemRequirement::GlibC(
                    Version::from_str("2.17").unwrap(),
                )),
                ..Default::default()
            }
        );
        assert_eq!(
            glibc_fam_2_17.union(&glibc_2_12).unwrap(),
            glibc_2_12.union(&glibc_fam_2_17).unwrap()
        );

        assert_eq!(
            glibc_fam_2_17.union(&libc_def_fam_2_19).unwrap(),
            SystemRequirements {
                libc: Some(LibCSystemRequirement::GlibC(
                    Version::from_str("2.19").unwrap()
                )),
                ..Default::default()
            }
        );

        assert_matches!(eglibc_2_17.union(&glibc_2_12).unwrap_err(), SystemRequirementsUnionError::DifferentLibcFamilies(fam_a, fam_b) if fam_a == "eglibc" && fam_b == "glibc");
    }
}
