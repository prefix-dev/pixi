use std::fmt;
use std::str::FromStr;

use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::{PrioritizedChannel, TomlError};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use serde::de::{Deserialize, Deserializer, Visitor};
use serde::Serialize;
use serde_with::{serde_as, serde_derive::Deserialize};
use thiserror::Error;

use super::environment::EnvironmentName;

use super::ExposedData;
use pixi_spec::PixiSpec;

/// Describes the contents of a parsed global project manifest.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedManifest {
    /// The environments the project can create.
    pub(crate) envs: IndexMap<EnvironmentName, ParsedEnvironment>,
}

impl<I> From<I> for ParsedManifest
where
    I: IntoIterator<Item = ExposedData>,
{
    fn from(value: I) -> Self {
        let mut envs: IndexMap<EnvironmentName, ParsedEnvironment> = IndexMap::new();
        for data in value {
            let ExposedData {
                env_name,
                platform,
                channel,
                package,
                executable_name,
                exposed,
            } = data;
            let parsed_environment = envs.entry(env_name).or_default();
            parsed_environment.channels.insert(channel);
            parsed_environment.platform = platform;
            parsed_environment
                .dependencies
                .insert(package, PixiSpec::default());
            parsed_environment.exposed.insert(exposed, executable_name);
        }

        Self { envs }
    }
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub(crate) fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_edit::de::from_str(source).map_err(TomlError::from)
    }
}

impl<'de> serde::Deserialize<'de> for ParsedManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize, Debug, Clone)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        pub struct TomlManifest {
            /// The environments the project can create.
            #[serde(default)]
            envs: IndexMap<EnvironmentName, ParsedEnvironment>,
        }

        let manifest = TomlManifest::deserialize(deserializer)?;

        // Check for duplicate keys in the exposed fields
        let mut exposed_names = IndexSet::new();
        let mut duplicates = IndexMap::new();
        for key in manifest.envs.values().flat_map(|env| env.exposed.keys()) {
            if !exposed_names.insert(key) {
                duplicates.entry(key).or_insert_with(Vec::new).push(key);
            }
        }
        if !duplicates.is_empty() {
            let duplicate_keys = duplicates.keys().map(|k| k.to_string()).collect_vec();
            return Err(serde::de::Error::custom(format!(
                "Duplicate exposed names found: '{}'",
                duplicate_keys.join(", ")
            )));
        }

        Ok(Self {
            envs: manifest.envs,
        })
    }
}

#[serde_as]
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ParsedEnvironment {
    #[serde_as(as = "IndexSet<pixi_manifest::TomlPrioritizedChannelStrOrMap>")]
    channels: IndexSet<pixi_manifest::PrioritizedChannel>,
    // Platform used by the environment.
    platform: Option<Platform>,
    #[serde(default, deserialize_with = "pixi_manifest::deserialize_package_map")]
    pub(crate) dependencies: IndexMap<PackageName, PixiSpec>,
    #[serde(default)]
    pub(crate) exposed: IndexMap<ExposedName, String>,
}

impl ParsedEnvironment {
    /// Returns the platform associated with this platform, `None` means current platform
    pub(crate) fn platform(&self) -> Option<Platform> {
        self.platform
    }

    /// Returns the channels associated with this environment.
    pub(crate) fn channels(&self) -> IndexSet<&NamedChannelOrUrl> {
        PrioritizedChannel::sort_channels_by_priority(&self.channels).collect()
    }

    /// Returns the dependencies associated with this environment.
    pub(crate) fn dependencies(&self) -> &IndexMap<PackageName, PixiSpec> {
        &self.dependencies
    }

    /// Returns the exposed names associated with this environment.
    pub(crate) fn exposed(&self) -> &IndexMap<ExposedName, String> {
        &self.exposed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub(crate) struct ExposedName(String);

impl fmt::Display for ExposedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ExposedName {
    type Err = miette::Report;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "pixi" {
            miette::bail!("The key 'pixi' is not allowed in the exposed map");
        } else {
            Ok(ExposedName(value.to_string()))
        }
    }
}

impl<'de> Deserialize<'de> for ExposedName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ExposedKeyVisitor;

        impl<'de> Visitor<'de> for ExposedKeyVisitor {
            type Value = ExposedName;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string that is not 'pixi'")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ExposedName::from_str(value).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(ExposedKeyVisitor)
    }
}

/// Represents an error that occurs when parsing an binary exposed name.
///
/// This error is returned when a string fails to be parsed as an environment name.
#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("pixi is not allowed as exposed name in the map")]
pub struct ParseExposedKeyError {}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::ParsedManifest;

    #[test]
    fn test_invalid_key() {
        let examples = [
            "[invalid]",
            "[envs.ipython.invalid]",
            "[envs.INVALID.dependencies]",
            "[envs.python_3.dependencies]",
        ];
        assert_snapshot!(examples
            .into_iter()
            .map(|example| ParsedManifest::from_toml_str(example)
                .unwrap_err()
                .to_string())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn test_duplicate_exposed() {
        let contents = r#"
        [envs.python-3-10]
        channels = ["conda-forge"]
        [envs.python-3-10.dependencies]
        python = "3.10"
        [envs.python-3-10.exposed]
        python = "python"
        python3 = "python"
        [envs.python-3-11]
        channels = ["conda-forge"]
        [envs.python-3-11.dependencies]
        python = "3.11"
        [envs.python-3-11.exposed]
        "python" = "python"
        "python3" = "python"
        "#;
        let manifest = ParsedManifest::from_toml_str(contents);

        assert!(manifest.is_err());
        assert_snapshot!(manifest.unwrap_err());
    }

    #[test]
    fn test_duplicate_dependency() {
        let contents = r#"
        [envs.python]
        channels = ["conda-forge"]
        [envs.python.dependencies]
        python = "*"
        PYTHON = "*"
        [envs.python.exposed]
        python = "python"
        "#;
        let manifest = ParsedManifest::from_toml_str(contents);

        assert!(manifest.is_err());
        assert_snapshot!(manifest.unwrap_err());
    }

    #[test]
    fn test_expose_pixi() {
        let contents = r#"
        [envs.test]
        channels = ["conda-forge"]
        [envs.test.dependencies]
        python = "*"
        [envs.test.exposed]
        pixi = "python"
        "#;
        let manifest = ParsedManifest::from_toml_str(contents);

        assert!(manifest.is_err());
        assert_snapshot!(manifest.unwrap_err());
    }

    #[test]
    fn test_tool_deserialization() {
        let contents = r#"
        # The name of the environment is `python`
        [envs.python]
        channels = ["conda-forge"]
        # optional, defaults to your current OS
        platform = "osx-64"
        # It will expose python, python3 and python3.11, but not pip
        [envs.python.dependencies]
        python = "3.11.*"
        pip = "*"

        [envs.python.exposed]
        python = "python"
        python3 = "python3"
        "python3.11" = "python3.11"

        # The name of the environment is `python3-10`
        [envs.python3-10]
        channels = ["https://fast.prefix.dev/conda-forge"]
        # It will expose python3.10
        [envs.python3-10.dependencies]
        python = "3.10.*"

        [envs.python3-10.exposed]
        "python3.10" = "python"
        "#;
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }
}
