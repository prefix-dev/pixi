use indexmap::IndexMap;
use pep440_rs::VersionSpecifiers;
use pep508_rs::VersionOrUrl;
use serde::de::{Error, MapAccess, Visitor};
use serde::{de, Deserialize, Deserializer};
use std::fmt::Formatter;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PyPiRequirement {
    version: Option<VersionSpecifiers>,
    extras: Option<Vec<String>>,
}

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error)]
pub enum ParsePyPiRequirementError {
    #[error("invalid PEP440")]
    Pep440Error(#[from] pep440_rs::Pep440Error),
}

impl FromStr for PyPiRequirement {
    type Err = ParsePyPiRequirementError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept a star as an any requirement, which is represented by the none.
        if s == "*" {
            Ok(Self {
                version: None,
                extras: None,
            })
        } else {
            // From string can only parse the version specifier.
            Ok(Self {
                version: Some(
                    VersionSpecifiers::from_str(s)
                        .map_err(ParsePyPiRequirementError::Pep440Error)?,
                ),
                extras: None,
            })
        }
    }
}

/// Represents a set of python dependencies on which a project can depend. The dependencies are
/// formatted using a custom version specifier.
#[derive(Default, Debug, Deserialize, Clone, Eq, PartialEq)]
pub struct PypiDependencies {
    #[serde(flatten)]
    requirements: IndexMap<rip::PackageName, PyPiRequirement>,
}

impl PypiDependencies {
    /// Returns `true` if no requirements have been specified
    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }

    /// Returns the requirements as [`pep508_rs::Requirement`]s.
    pub fn as_pep508(&self) -> Vec<pep508_rs::Requirement> {
        self.requirements
            .iter()
            .map(|(name, req)| {
                let version = req.version.clone().map(VersionOrUrl::VersionSpecifier);

                pep508_rs::Requirement {
                    name: name.as_str().to_string(),
                    extras: req.extras.clone(),
                    version_or_url: version,
                    marker: None,
                }
            })
            .collect()
    }
}
impl<'de> Deserialize<'de> for PyPiRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequirementVisitor;
        impl<'de> Visitor<'de> for RequirementVisitor {
            type Value = PyPiRequirement;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("a mapping from package names to a pypi requirement")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                PyPiRequirement::from_str(v).map_err(Error::custom)
            }
            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                // Use a temp struct to deserialize into when it is a map.
                #[derive(Deserialize)]
                struct RawPyPiRequirement {
                    version: Option<VersionSpecifiers>,
                    extras: Option<Vec<String>>,
                }
                let raw_requirement =
                    RawPyPiRequirement::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(PyPiRequirement {
                    version: raw_requirement.version,
                    extras: raw_requirement.extras,
                })
            }
        }
        deserializer.deserialize_any(RequirementVisitor {})
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_only_version() {
        let requirement: PypiDependencies = toml_edit::de::from_str(r#"foo = ">=3.12""#).unwrap();
        assert_eq!(
            requirement,
            PypiDependencies {
                requirements: IndexMap::from([(
                    rip::PackageName::from_str("foo").unwrap(),
                    PyPiRequirement {
                        version: Some(VersionSpecifiers::from_str(">=3.12").unwrap()),
                        extras: None
                    }
                ),])
            }
        );
        let requirement: PypiDependencies = toml_edit::de::from_str(r#"foo = "==3.12.0""#).unwrap();
        assert_eq!(
            requirement,
            PypiDependencies {
                requirements: IndexMap::from([(
                    rip::PackageName::from_str("foo").unwrap(),
                    PyPiRequirement {
                        version: Some(VersionSpecifiers::from_str("==3.12.0").unwrap()),
                        extras: None
                    }
                ),])
            }
        );
        let requirement: PypiDependencies = toml_edit::de::from_str(r#"foo = "~=2.1.3""#).unwrap();
        assert_eq!(
            requirement,
            PypiDependencies {
                requirements: IndexMap::from([(
                    rip::PackageName::from_str("foo").unwrap(),
                    PyPiRequirement {
                        version: Some(VersionSpecifiers::from_str("~=2.1.3").unwrap()),
                        extras: None
                    }
                ),])
            }
        );
        let requirement: PypiDependencies = toml_edit::de::from_str(r#"foo = "*""#).unwrap();
        assert_eq!(
            requirement,
            PypiDependencies {
                requirements: IndexMap::from([(
                    rip::PackageName::from_str("foo").unwrap(),
                    PyPiRequirement {
                        version: None,
                        extras: None
                    }
                ),])
            }
        );
    }

    #[test]
    fn test_extended() {
        let requirement: PypiDependencies =
            toml::de::from_str(r#"foo = { version=">=3.12", extras = ["bar"] }"#).unwrap();
        assert_eq!(
            requirement,
            PypiDependencies {
                requirements: IndexMap::from([(
                    rip::PackageName::from_str("foo").unwrap(),
                    PyPiRequirement {
                        version: Some(VersionSpecifiers::from_str(">=3.12").unwrap()),
                        extras: Some(vec!("bar".to_string()))
                    }
                ),])
            }
        );

        let requirement: PypiDependencies =
            toml::de::from_str(r#"bar = { version=">=3.12,<3.13.0", extras = ["bar", "foo"] }"#)
                .unwrap();
        assert_eq!(
            requirement,
            PypiDependencies {
                requirements: IndexMap::from([(
                    rip::PackageName::from_str("bar").unwrap(),
                    PyPiRequirement {
                        version: Some(VersionSpecifiers::from_str(">=3.12,<3.13.0").unwrap()),
                        extras: Some(vec!("bar".to_string(), "foo".to_string()))
                    }
                ),])
            }
        );
    }
}
