use crate::consts;
use crate::utils::spanned::PixiSpanned;
use lazy_static::lazy_static;
use miette::Diagnostic;
use regex::Regex;
use serde::{self, Deserialize, Deserializer};
use std::borrow::Borrow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use thiserror::Error;

/// The name of an environment. This is either a string or default for the default environment.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvironmentName {
    Default,
    Named(String),
}

impl Hash for EnvironmentName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl EnvironmentName {
    /// Returns the name of the environment. This is either the name of the environment or the name
    /// of the default environment.
    pub fn as_str(&self) -> &str {
        match self {
            EnvironmentName::Default => consts::DEFAULT_ENVIRONMENT_NAME,
            EnvironmentName::Named(name) => name.as_str(),
        }
    }
}

impl Borrow<str> for EnvironmentName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for EnvironmentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvironmentName::Default => write!(f, "{}", consts::DEFAULT_ENVIRONMENT_NAME),
            EnvironmentName::Named(name) => write!(f, "{}", name),
        }
    }
}

impl PartialEq<str> for EnvironmentName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("Failed to parse environment name '{attempted_parse}', please use only lowercase letters, numbers and dashes")]
pub struct ParseEnvironmentNameError {
    /// The string that was attempted to be parsed.
    pub attempted_parse: String,
}

impl FromStr for EnvironmentName {
    type Err = ParseEnvironmentNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        lazy_static! {
            static ref REGEX: Regex = Regex::new(r"^[a-z0-9-]+$").expect("Regex should be able to compile"); // Compile the regex
        }

        if !REGEX.is_match(s) {
            // Return an error if the string does not match the regex
            return Err(ParseEnvironmentNameError {
                attempted_parse: s.to_string(),
            });
        }
        match s {
            consts::DEFAULT_ENVIRONMENT_NAME => Ok(EnvironmentName::Default),
            _ => Ok(EnvironmentName::Named(s.to_string())),
        }
    }
}

impl<'de> Deserialize<'de> for EnvironmentName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        EnvironmentName::from_str(&name).map_err(serde::de::Error::custom)
    }
}

/// An environment describes a set of features that are available together.
///
/// Individual features cannot be used directly, instead they are grouped together into
/// environments. Environments are then locked and installed.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The name of the environment
    pub name: EnvironmentName,

    /// The names of the features that together make up this environment.
    ///
    /// Note that the default feature is always added to the set of features that make up the
    /// environment.
    pub features: Vec<String>,

    /// The optional location of where the features of the environment are defined in the manifest toml.
    pub features_source_loc: Option<std::ops::Range<usize>>,

    /// An optional solver-group. Multiple environments can share the same solve-group. All the
    /// dependencies of the environment that share the same solve-group will be solved together.
    pub solve_group: Option<usize>,
}

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct TomlEnvironment {
    #[serde(default)]
    pub features: PixiSpanned<Vec<String>>,
    pub solve_group: Option<String>,
}

pub(super) enum TomlEnvironmentMapOrSeq {
    Map(TomlEnvironment),
    Seq(Vec<String>),
}

impl<'de> Deserialize<'de> for TomlEnvironmentMapOrSeq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .map(|map| map.deserialize().map(TomlEnvironmentMapOrSeq::Map))
            .seq(|seq| seq.deserialize().map(TomlEnvironmentMapOrSeq::Seq))
            .expecting("either a map or a sequence")
            .deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_environment_name_from_str() {
        assert_eq!(
            EnvironmentName::from_str("default").unwrap(),
            EnvironmentName::Default
        );
        assert_eq!(
            EnvironmentName::from_str("foo").unwrap(),
            EnvironmentName::Named("foo".to_string())
        );
        assert_eq!(
            EnvironmentName::from_str("foo_bar").unwrap_err(),
            ParseEnvironmentNameError {
                attempted_parse: "foo_bar".to_string()
            }
        );

        assert!(EnvironmentName::from_str("foo-bar").is_ok());
        assert!(EnvironmentName::from_str("foo1").is_ok());
        assert!(EnvironmentName::from_str("py39").is_ok());

        assert!(EnvironmentName::from_str("foo bar").is_err());
        assert!(EnvironmentName::from_str("foo_bar").is_err());
        assert!(EnvironmentName::from_str("foo/bar").is_err());
        assert!(EnvironmentName::from_str("foo\\bar").is_err());
        assert!(EnvironmentName::from_str("foo:bar").is_err());
        assert!(EnvironmentName::from_str("foo;bar").is_err());
        assert!(EnvironmentName::from_str("foo?bar").is_err());
        assert!(EnvironmentName::from_str("foo!bar").is_err());
        assert!(EnvironmentName::from_str("py3.9").is_err());
        assert!(EnvironmentName::from_str("py-3.9").is_err());
        assert!(EnvironmentName::from_str("py_3.9").is_err());
        assert!(EnvironmentName::from_str("py/3.9").is_err());
        assert!(EnvironmentName::from_str("py\\3.9").is_err());
        assert!(EnvironmentName::from_str("Py").is_err());
        assert!(EnvironmentName::from_str("Py3").is_err());
        assert!(EnvironmentName::from_str("Py39").is_err());
        assert!(EnvironmentName::from_str("Py-39").is_err());
    }

    #[test]
    fn test_environment_name_as_str() {
        assert_eq!(EnvironmentName::Default.as_str(), "default");
        assert_eq!(EnvironmentName::Named("foo".to_string()).as_str(), "foo");
    }

    #[test]
    fn test_deserialize_environment_name() {
        assert_eq!(
            serde_json::from_str::<EnvironmentName>("\"default\"").unwrap(),
            EnvironmentName::Default
        );
        assert_eq!(
            serde_json::from_str::<EnvironmentName>("\"foo\"").unwrap(),
            EnvironmentName::Named("foo".to_string())
        );
    }
}
