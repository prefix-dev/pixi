use std::collections::BTreeMap;

use miette::Diagnostic;
use rattler_conda_types::Version;
use rattler_virtual_packages::{Cuda, LibC, Linux, Osx, VirtualPackage};
use serde::Serialize;
use serde_value::Value;
use thiserror::Error;

pub const GLIBC_FAMILY: &str = "glibc";
pub const MUSL_FAMILY: &str = "musl";

/// Describes the minimal system requirements to be able to run a certain environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SystemRequirements {
    /// Dictates the minimum version of macOS required.
    pub macos: Option<Version>,

    /// Dictates the minimum linux version required.
    pub linux: Option<Version>,

    /// Dictates the minimum cuda version required.
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

    /// Returns the combination of two system requirements.
    ///
    /// If both system requirements specify the same virtual package, the incoming version is taken.
    ///
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            linux: other.linux.clone().or(self.linux.clone()),
            cuda: other.cuda.clone().or(self.cuda.clone()),
            macos: other.macos.clone().or(self.macos.clone()),
            libc: other.libc.clone().or(self.libc.clone()),
            archspec: other.archspec.clone().or(self.archspec.clone()),
        }
    }

    /// Returns true if the system requirements are empty, meaning that no requirements were specified.
    pub fn is_empty(&self) -> bool {
        self.linux.is_none()
            && self.cuda.is_none()
            && self.macos.is_none()
            && self.libc.is_none()
            && self.archspec.is_none()
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

impl Serialize for LibCSystemRequirement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            LibCSystemRequirement::GlibC(version) => serializer.serialize_str(&version.to_string()),
            LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion { family, version }) => {
                let mut map = BTreeMap::new(); // Initialize the BTreeMap
                if let Some(fam) = family {
                    map.insert(
                        Value::String("family".to_string()),
                        Value::String(fam.clone()),
                    );
                }
                map.insert(
                    Value::String("version".to_string()),
                    Value::String(version.to_string()),
                );
                Value::Map(map).serialize(serializer) // Wrap the map in Value::Map
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct LibCFamilyAndVersion {
    /// The libc family, e.g. glibc
    pub family: Option<String>,

    /// The minimum version of the libc family
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

impl std::fmt::Display for LibCSystemRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibCSystemRequirement::GlibC(version) => {
                write!(f, "GlibC version: {version}")
            }
            LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion { family, version }) => {
                match family {
                    Some(fam) => write!(f, "{fam} version: {version}"),
                    None => write!(f, "No family, version: {version}"),
                }
            }
        }
    }
}

impl std::fmt::Display for SystemRequirements {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "- {} {}",
            console::style("macOS:").cyan(),
            self.macos
                .as_ref()
                .map_or("None".to_string(), |v| v.to_string())
        )?;
        writeln!(
            f,
            "- {} {}",
            console::style("Linux:").cyan(),
            self.linux
                .as_ref()
                .map_or("None".to_string(), |v| v.to_string())
        )?;
        writeln!(
            f,
            "- {} {}",
            console::style("CUDA:").cyan(),
            self.cuda
                .as_ref()
                .map_or("None".to_string(), |v| v.to_string())
        )?;
        writeln!(
            f,
            "- {} {}",
            console::style("LibC:").cyan(),
            self.libc
                .as_ref()
                .map_or("None".to_string(), |v| v.to_string())
        )?;
        writeln!(
            f,
            "- {} {}",
            console::style("Archspec:").cyan(),
            self.archspec.as_deref().unwrap_or("None")
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use rattler_conda_types::Version;
    use std::str::FromStr;
    use toml_edit::ser::to_string_pretty;

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

        assert_matches!(eglibc_2_17.union(&glibc_2_12).unwrap_err(), SystemRequirementsUnionError::DifferentLibcFamilies(fam_a, fam_b) if fam_a == "eglibc" && fam_b == GLIBC_FAMILY);
    }

    #[test]
    fn test_serialization() {
        let system_requirements = SystemRequirements {
            macos: Some(Version::from_str("10.15").unwrap()),
            linux: Some(Version::from_str("5.11").unwrap()),
            cuda: Some(Version::from_str("12.2").unwrap()),
            libc: Some(LibCSystemRequirement::GlibC(
                Version::from_str("2.12").unwrap(),
            )),
            archspec: Some("x86_64".to_string()),
        };

        let serialized = to_string_pretty(&system_requirements).unwrap();
        assert_snapshot!(serialized);
    }

    #[test]
    fn test_serialization_other_family() {
        let system_requirements = SystemRequirements {
            macos: Some(Version::from_str("10.15").unwrap()),
            linux: Some(Version::from_str("5.11").unwrap()),
            cuda: Some(Version::from_str("12.2").unwrap()),
            libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: Some(GLIBC_FAMILY.to_string()),
                version: Version::from_str("2.12").unwrap(),
            })),
            archspec: Some("x86_64".to_string()),
        };

        let serialized = to_string_pretty(&system_requirements).unwrap();
        assert_snapshot!(serialized);
    }

    #[test]
    fn test_merge() {
        let a = SystemRequirements {
            macos: Some(Version::from_str("10.15").unwrap()),
            linux: Some(Version::from_str("5.11").unwrap()),
            cuda: Some(Version::from_str("12.2").unwrap()),
            libc: Some(LibCSystemRequirement::GlibC(
                Version::from_str("2.12").unwrap(),
            )),
            archspec: Some("x86_64".to_string()),
        };

        let b = SystemRequirements {
            macos: Some(Version::from_str("10.16").unwrap()),
            linux: Some(Version::from_str("5.12").unwrap()),
            cuda: Some(Version::from_str("12.1").unwrap()),
            libc: Some(LibCSystemRequirement::GlibC(
                Version::from_str("2.13").unwrap(),
            )),
            archspec: Some("arm".to_string()),
        };

        let c = a.merge(&b);

        assert_eq!(c.macos, Some(Version::from_str("10.16").unwrap()));
        assert_eq!(c.linux, Some(Version::from_str("5.12").unwrap()));
        assert_eq!(c.cuda, Some(Version::from_str("12.1").unwrap()));
        assert_eq!(
            c.libc,
            Some(LibCSystemRequirement::GlibC(
                Version::from_str("2.13").unwrap()
            ))
        );
        assert_eq!(c.archspec, Some("arm".to_string()));

        let d = SystemRequirements {
            macos: None,
            linux: None,
            cuda: None,
            libc: Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: Some(MUSL_FAMILY.to_string()),
                version: Version::from_str("2.13").unwrap(),
            })),
            archspec: None,
        };

        let e = a.merge(&d);

        assert_eq!(e.macos, Some(Version::from_str("10.15").unwrap()));
        assert_eq!(e.linux, Some(Version::from_str("5.11").unwrap()));
        assert_eq!(e.cuda, Some(Version::from_str("12.2").unwrap()));
        assert_eq!(
            e.libc,
            Some(LibCSystemRequirement::OtherFamily(LibCFamilyAndVersion {
                family: Some(MUSL_FAMILY.to_string()),
                version: Version::from_str("2.13").unwrap(),
            }))
        );
        assert_eq!(e.archspec, Some("x86_64".to_string()));
    }
}
