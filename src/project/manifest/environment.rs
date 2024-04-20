use crate::consts;
use crate::utils::spanned::PixiSpanned;
use lazy_static::lazy_static;
use miette::Diagnostic;
use regex::Regex;
use serde::{self, Deserialize, Deserializer};
use serde_with::SerializeDisplay;
use std::borrow::Borrow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use thiserror::Error;

/// The name of an environment. This is either a string or default for the default environment.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, SerializeDisplay)]
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

    /// Returns true if the environment is the default environment.
    pub fn is_default(&self) -> bool {
        matches!(self, EnvironmentName::Default)
    }

    /// Returns a styled version of the environment name for display in the console.
    pub fn fancy_display(&self) -> console::StyledObject<&str> {
        console::style(self.as_str()).magenta()
    }

    /// Tries to read the environment name from an argument, then it will try
    /// to read from an environment variable, otherwise it will fall back to default
    pub fn from_arg_or_env_var(arg_name: Option<String>) -> Self {
        if let Some(arg_name) = arg_name {
            return EnvironmentName::Named(arg_name);
        } else if std::env::var("PIXI_IN_SHELL").is_ok() {
            if let Ok(env_var_name) = std::env::var("PIXI_ENVIRONMENT_NAME") {
                if env_var_name == consts::DEFAULT_ENVIRONMENT_NAME {
                    return EnvironmentName::Default;
                }
                return EnvironmentName::Named(env_var_name);
            }
        }
        EnvironmentName::Default
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

    /// Components to include from the default feature
    pub from_default_feature: FromDefaultFeature,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            name: EnvironmentName::Default,
            features: Vec::new(),
            features_source_loc: None,
            solve_group: None,
            from_default_feature: FromDefaultFeature::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FromDefaultFeature {
    pub system_requirements: bool,
    pub channels: bool,
    pub platforms: bool,
    pub dependencies: bool,
    pub pypi_dependencies: bool,
    pub activation: bool,
    pub tasks: bool,
}

// by default, include everything from the default feature
impl Default for FromDefaultFeature {
    fn default() -> Self {
        Self {
            system_requirements: true,
            channels: true,
            platforms: true,
            dependencies: true,
            pypi_dependencies: true,
            activation: true,
            tasks: true,
        }
    }
}

/// Deserialisation conversion helper to get a FromDefaultFeature from TOML environment data
impl From<Option<FromDefaultToml>> for FromDefaultFeature {
    fn from(opt: Option<FromDefaultToml>) -> Self {
        match opt {
            None => FromDefaultFeature::default(),
            Some(FromDefaultToml::IncludeFromDefault(included)) => {
                let mut f = FromDefaultFeature {
                    system_requirements: false,
                    channels: false,
                    platforms: false,
                    dependencies: false,
                    pypi_dependencies: false,
                    activation: false,
                    tasks: false,
                };

                for component in &included {
                    match component {
                        FeatureComponentToml::SystemRequirements => f.system_requirements = true,
                        FeatureComponentToml::Channels => f.channels = true,
                        FeatureComponentToml::Platforms => f.platforms = true,
                        FeatureComponentToml::Dependencies => f.dependencies = true,
                        FeatureComponentToml::PypiDependencies => f.pypi_dependencies = true,
                        FeatureComponentToml::Activation => f.activation = true,
                        FeatureComponentToml::Tasks => f.tasks = true,
                    }
                }

                f
            }
            Some(FromDefaultToml::ExcludeFromDefault(excluded)) => {
                let mut f = FromDefaultFeature::default();
                for component in &excluded {
                    match component {
                        FeatureComponentToml::SystemRequirements => f.system_requirements = false,
                        FeatureComponentToml::Channels => f.channels = false,
                        FeatureComponentToml::Platforms => f.platforms = false,
                        FeatureComponentToml::Dependencies => f.dependencies = false,
                        FeatureComponentToml::PypiDependencies => f.pypi_dependencies = false,
                        FeatureComponentToml::Activation => f.activation = false,
                        FeatureComponentToml::Tasks => f.tasks = false,
                    }
                }

                f
            }
        }
    }
}

/// Helper struct to deserialize the environment from TOML.
/// The environment description can only hold these values.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct TomlEnvironment {
    #[serde(default)]
    pub features: PixiSpanned<Vec<String>>,
    pub solve_group: Option<String>,
    #[serde(flatten)]
    pub from_default: Option<FromDefaultToml>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FromDefaultToml {
    IncludeFromDefault(Vec<FeatureComponentToml>),
    ExcludeFromDefault(Vec<FeatureComponentToml>),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FeatureComponentToml {
    SystemRequirements,
    Channels,
    Platforms,
    Dependencies,
    PypiDependencies,
    Activation,
    Tasks,
}

#[derive(Debug)]
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
    use indexmap::IndexMap;

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

    fn from_default(source: &str) -> FromDefaultFeature {
        let env =
            toml_edit::de::from_str::<IndexMap<EnvironmentName, TomlEnvironmentMapOrSeq>>(source)
                .unwrap();
        match env.values().next().unwrap() {
            TomlEnvironmentMapOrSeq::Map(env) => env.from_default.clone().into(),
            TomlEnvironmentMapOrSeq::Seq(_) => FromDefaultFeature::default(),
        }
    }

    #[test]
    fn test_deserialize_exclude_from_default() {
        let source = r#"test = { exclude-from-default=["channels"]}"#;
        assert_eq!(false, from_default(source).channels);
        assert_eq!(true, from_default(source).platforms);
    }
    #[test]
    fn test_deserialize_include_from_default() {
        let source = r#"test = { include-from-default=["channels"]}"#;
        assert_eq!(true, from_default(source).channels);
        assert_eq!(false, from_default(source).platforms);
    }
    #[test]
    fn test_deserialize_no_from_default() {
        let source = r#"test = ["bla"]"#;
        assert_eq!(true, from_default(source).channels);
        assert_eq!(true, from_default(source).platforms);
    }
    #[test]
    #[should_panic(expected = "unknown field `exclude-from-default`")]
    fn test_deserialize_from_default_conflict() {
        let source =
            r#"test = { include-from-default=["channels"], exclude-from-default=["platform"]}"#;
        from_default(source);
        ()
    }
}
