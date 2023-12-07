use pep440_rs::VersionSpecifiers;
use serde::de::{Error, MapAccess, Visitor};
use serde::{de, Deserialize, Deserializer};
use std::fmt::Formatter;
use std::str::FromStr;
use thiserror::Error;
use toml_edit::Item;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PyPiRequirement {
    pub(crate) version: Option<pep440_rs::VersionSpecifiers>,
    pub(crate) extras: Option<Vec<String>>,
}

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error)]
pub enum ParsePyPiRequirementError {
    #[error("invalid pep440 version specifier")]
    Pep440Error(#[from] pep440_rs::Pep440Error),

    #[error("empty string is not allowed, did you mean '*'?")]
    EmptyStringNotAllowed,

    #[error("missing operator in version specifier, did you mean '~={0}'?")]
    MissingOperator(String),
}

impl From<PyPiRequirement> for Item {
    /// PyPiRequirement to a toml_edit item, to put in the manifest file.
    fn from(val: PyPiRequirement) -> Item {
        if val.extras.is_some() {
            // If extras is defined use an inline table
            let mut table = toml_edit::Table::new().into_inline_table();

            // First add the version
            if val.version.is_some() {
                let v = val.version.expect("Expect a version here").to_string();
                table.insert(
                    "version",
                    toml_edit::Value::String(toml_edit::Formatted::new(v)),
                );
            } else {
                table.insert(
                    "version",
                    toml_edit::Value::String(toml_edit::Formatted::new("*".to_string())),
                );
            }
            // Add extras as an array.
            table.insert(
                "extras",
                toml_edit::Value::Array(toml_edit::Array::from_iter(val.extras.unwrap())),
            );
            Item::Value(toml_edit::Value::InlineTable(table))
        } else {
            // Without extras use the string representation.
            if val.version.is_some() {
                Item::Value(val.version.unwrap().to_string().into())
            } else {
                Item::Value("*".into())
            }
        }
    }
}
impl FromStr for PyPiRequirement {
    type Err = ParsePyPiRequirementError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s.is_empty() {
            return Err(ParsePyPiRequirementError::EmptyStringNotAllowed);
        }
        if s == "*" {
            // Accept a star as an any requirement, which is represented by the none.
            Ok(Self {
                version: None,
                extras: None,
            })
        } else if s.starts_with(|c: char| c.is_ascii_digit()) {
            Err(ParsePyPiRequirementError::MissingOperator(s.to_string()))
        } else {
            // From string can only parse the version specifier.
            Ok(Self {
                version: Some(
                    pep440_rs::VersionSpecifiers::from_str(s)
                        .map_err(ParsePyPiRequirementError::Pep440Error)?,
                ),
                extras: None,
            })
        }
    }
}

/// Implement from [`pep508_rs::Requirement`] to make the conversion easier.
impl From<pep508_rs::Requirement> for PyPiRequirement {
    fn from(req: pep508_rs::Requirement) -> Self {
        let version = if let Some(version_or_url) = req.version_or_url {
            match version_or_url {
                pep508_rs::VersionOrUrl::VersionSpecifier(v) => Some(v),
                pep508_rs::VersionOrUrl::Url(_) => None,
            }
        } else {
            None
        };
        PyPiRequirement {
            version,
            extras: req.extras,
        }
    }
}

impl PyPiRequirement {
    /// Returns the requirements as [`pep508_rs::Requirement`]s.
    pub fn as_pep508(&self, name: &rip::types::PackageName) -> pep508_rs::Requirement {
        pep508_rs::Requirement {
            name: name.as_str().to_string(),
            extras: self.extras.clone(),
            version_or_url: self
                .version
                .clone()
                .map(pep508_rs::VersionOrUrl::VersionSpecifier),
            marker: None,
        }
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
                    version: Option<String>,
                    extras: Option<Vec<String>>,
                }
                let raw_requirement =
                    RawPyPiRequirement::deserialize(de::value::MapAccessDeserializer::new(map))?;

                // Parse the * in version or allow for no version with extras.
                let mut version = None;
                if let Some(raw_version) = raw_requirement.version {
                    if raw_version != "*" {
                        version = Some(
                            VersionSpecifiers::from_str(raw_version.as_str())
                                .map_err(A::Error::custom)?,
                        );
                    }
                }
                Ok(PyPiRequirement {
                    version,
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
    use indexmap::IndexMap;

    #[test]
    fn test_only_version() {
        let requirement: IndexMap<rip::types::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = ">=3.12""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &rip::types::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                version: Some(pep440_rs::VersionSpecifiers::from_str(">=3.12").unwrap()),
                extras: None
            }
        );
        let requirement: IndexMap<rip::types::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "==3.12.0""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                version: Some(pep440_rs::VersionSpecifiers::from_str("==3.12.0").unwrap()),
                extras: None
            }
        );

        let requirement: IndexMap<rip::types::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "~=2.1.3""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                version: Some(pep440_rs::VersionSpecifiers::from_str("~=2.1.3").unwrap()),
                extras: None
            }
        );

        let requirement: IndexMap<rip::types::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = "*""#).unwrap();
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                version: None,
                extras: None
            }
        );
    }

    #[test]
    fn test_extended() {
        let requirement: IndexMap<rip::types::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(r#"foo = { version=">=3.12", extras = ["bar"] }"#).unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &rip::types::PackageName::from_str("foo").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                version: Some(pep440_rs::VersionSpecifiers::from_str(">=3.12").unwrap()),
                extras: Some(vec!("bar".to_string()))
            }
        );

        let requirement: IndexMap<rip::types::PackageName, PyPiRequirement> =
            toml_edit::de::from_str(
                r#"bar = { version=">=3.12,<3.13.0", extras = ["bar", "foo"] }"#,
            )
            .unwrap();
        assert_eq!(
            requirement.first().unwrap().0,
            &rip::types::PackageName::from_str("bar").unwrap()
        );
        assert_eq!(
            requirement.first().unwrap().1,
            &PyPiRequirement {
                version: Some(pep440_rs::VersionSpecifiers::from_str(">=3.12,<3.13.0").unwrap()),
                extras: Some(vec!("bar".to_string(), "foo".to_string()))
            }
        );
    }
}
