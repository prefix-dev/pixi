use std::{
    borrow::Borrow,
    fmt,
    hash::{Hash, Hasher},
    str::FromStr,
};

use miette::Diagnostic;
use regex::Regex;
use serde::{self, Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{consts::DEFAULT_ENVIRONMENT_NAME, solve_group::SolveGroupIdx};

#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error(
    "Failed to parse environment name '{attempted_parse}', please use only lowercase letters, numbers and dashes"
)]
pub struct ParseEnvironmentNameError {
    /// The string that was attempted to be parsed.
    pub attempted_parse: String,
}

/// The name of an environment. This is either a string or default for the
/// default environment.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvironmentName {
    #[default]
    Default,
    Named(String),
}

impl Serialize for EnvironmentName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl Hash for EnvironmentName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl EnvironmentName {
    /// Returns the name of the environment. This is either the name of the
    /// environment or the name of the default environment.
    pub fn as_str(&self) -> &str {
        match self {
            EnvironmentName::Default => DEFAULT_ENVIRONMENT_NAME,
            EnvironmentName::Named(name) => name.as_str(),
        }
    }

    /// Returns true if the environment is the default environment.
    pub fn is_default(&self) -> bool {
        matches!(self, EnvironmentName::Default)
    }

    /// Tries to read the environment name from an argument, then it will try
    /// to read from an environment variable, otherwise it will fall back to
    /// default
    pub fn from_arg_or_env_var(
        arg_name: Option<String>,
    ) -> Result<Self, ParseEnvironmentNameError> {
        if let Some(arg_name) = arg_name {
            return EnvironmentName::from_str(&arg_name);
        } else if std::env::var("PIXI_IN_SHELL").is_ok()
            && let Ok(env_var_name) = std::env::var("PIXI_ENVIRONMENT_NAME")
        {
            if env_var_name == DEFAULT_ENVIRONMENT_NAME {
                return Ok(EnvironmentName::Default);
            }
            return Ok(EnvironmentName::Named(env_var_name));
        }
        Ok(EnvironmentName::Default)
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
            EnvironmentName::Default => write!(f, "{DEFAULT_ENVIRONMENT_NAME}"),
            EnvironmentName::Named(name) => write!(f, "{name}"),
        }
    }
}

impl PartialEq<str> for EnvironmentName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl FromStr for EnvironmentName {
    type Err = ParseEnvironmentNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        static REGEX: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let regex = REGEX
            .get_or_init(|| Regex::new(r"^[a-z0-9-]+$").expect("Regex should be able to compile"));

        if !regex.is_match(s) {
            // Return an error if the string does not match the regex
            return Err(ParseEnvironmentNameError {
                attempted_parse: s.to_string(),
            });
        }
        match s {
            DEFAULT_ENVIRONMENT_NAME => Ok(EnvironmentName::Default),
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
/// Individual features cannot be used directly, instead they are grouped
/// together into environments. Environments are then locked and installed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Environment {
    /// The name of the environment
    pub name: EnvironmentName,

    /// The names of the features that together make up this environment.
    ///
    /// Note that the default feature is always added to the set of features
    /// that make up the environment.
    pub features: Vec<EnvironmentFeature>,

    /// An optional solver-group. Multiple environments can share the same
    /// solve-group. All the dependencies of the environment that share the
    /// same solve-group will be solved together.
    pub solve_group: Option<SolveGroupIdx>,

    /// Whether to include the default feature in that environment
    pub no_default_feature: bool,
}

/// Describes how a feature is referenced by an environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentFeature {
    /// An inline feature defined directly on the environment.
    /// Resolves using the parent environment's name.
    Inline,

    /// A named feature defined in a `[feature.*]` section.
    Named(String),
}

impl EnvironmentFeature {
    /// Returns `true` if this is an inline feature.
    pub fn is_inline(&self) -> bool {
        matches!(self, EnvironmentFeature::Inline)
    }

    /// Returns the feature name if this is a named feature.
    pub fn as_named(&self) -> Option<&str> {
        match self {
            EnvironmentFeature::Named(name) => Some(name),
            EnvironmentFeature::Inline => None,
        }
    }

    /// Returns the key used to store/lookup the feature in the features map.
    /// Inline features use a dot-prefix to avoid collisions with named features.
    pub fn to_feature_name(&self, environment_name: &EnvironmentName) -> String {
        match self {
            EnvironmentFeature::Inline => format!(".{}", environment_name.as_str()),
            EnvironmentFeature::Named(name) => name.clone(),
        }
    }
}

impl fmt::Display for EnvironmentFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvironmentFeature::Inline => write!(f, "<inline>"),
            EnvironmentFeature::Named(name) => write!(f, "{name}"),
        }
    }
}

impl Serialize for EnvironmentFeature {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            EnvironmentFeature::Inline => serializer.serialize_none(),
            EnvironmentFeature::Named(name) => serializer.serialize_str(name),
        }
    }
}

impl<'de> Deserialize<'de> for EnvironmentFeature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = EnvironmentFeature;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "null or a feature name string")
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(EnvironmentFeature::Inline)
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(EnvironmentFeature::Inline)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(EnvironmentFeature::Named(v.to_string()))
            }

            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(EnvironmentFeature::Named(v))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl From<String> for EnvironmentFeature {
    fn from(name: String) -> Self {
        EnvironmentFeature::Named(name)
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

    #[test]
    fn test_environment_feature_serialization() {
        let inline = EnvironmentFeature::Inline;
        let named = EnvironmentFeature::Named("test".to_string());

        // Round-trip through JSON
        let inline_json = serde_json::to_string(&inline).unwrap();
        let named_json = serde_json::to_string(&named).unwrap();

        assert_eq!(inline_json, "null");
        assert_eq!(named_json, "\"test\"");

        let inline_back: EnvironmentFeature = serde_json::from_str(&inline_json).unwrap();
        let named_back: EnvironmentFeature = serde_json::from_str(&named_json).unwrap();

        assert_eq!(inline_back, EnvironmentFeature::Inline);
        assert_eq!(named_back, EnvironmentFeature::Named("test".to_string()));
    }

    #[test]
    fn test_environment_feature_helpers() {
        let inline = EnvironmentFeature::Inline;
        let named = EnvironmentFeature::Named("foo".to_string());

        assert!(inline.is_inline());
        assert!(!named.is_inline());

        assert_eq!(inline.as_named(), None);
        assert_eq!(named.as_named(), Some("foo"));

        let env_name = EnvironmentName::Named("dev".to_string());
        assert_eq!(inline.to_feature_name(&env_name), ".dev");
        assert_eq!(named.to_feature_name(&env_name), "foo");
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct EnvironmentIdx(pub(crate) usize);
