use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use super::environment::EnvironmentName;
use super::ExposedData;
use crate::global::Mapping;
use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{Context, Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use pixi_consts::consts;
use pixi_manifest::utils::package_map::UniquePackageMap;
use pixi_manifest::PrioritizedChannel;
use pixi_spec::PixiSpec;
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use serde::de::{Deserialize, Deserializer, Visitor};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_with::{serde_as, serde_derive::Deserialize};
use thiserror::Error;
use toml_edit::TomlError;

pub const GLOBAL_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ManifestVersion(u32);

impl Default for ManifestVersion {
    fn default() -> Self {
        ManifestVersion(GLOBAL_MANIFEST_VERSION)
    }
}

impl From<ManifestVersion> for toml_edit::Item {
    fn from(version: ManifestVersion) -> Self {
        toml_edit::value(version.0 as i64)
    }
}

#[derive(Error, Debug, Clone, Diagnostic)]
pub enum ManifestParsingError {
    #[error(transparent)]
    Error(#[from] toml_edit::TomlError),
    #[error("The 'version' of the manifest is too low: '{0}', the supported version is '{GLOBAL_MANIFEST_VERSION}', please update the manifest")]
    VersionTooLow(u32, #[source] toml_edit::TomlError),
    #[error("The 'version' of the manifest is too high: '{0}', the supported version is '{GLOBAL_MANIFEST_VERSION}', please update `pixi` to support the new manifest version")]
    VersionTooHigh(u32, #[source] toml_edit::TomlError),
}

impl ManifestParsingError {
    pub fn to_fancy<T>(
        &self,
        file_name: &str,
        contents: impl Into<String>,
        path: &Path,
    ) -> Result<T, Report> {
        if let Some(span) = self.span() {
            Err(miette::miette!(
                labels = vec![LabeledSpan::at(span, self.message())],
                "Failed to parse global manifest: {}",
                console::style(path.display()).bold()
            )
            .with_source_code(NamedSource::new(file_name, contents.into())))
        } else {
            Err(self.clone()).into_diagnostic().with_context(|| {
                format!(
                    "Failed to parse global manifest: '{}'",
                    console::style(path.display()).bold()
                )
            })
        }
    }

    fn span(&self) -> Option<std::ops::Range<usize>> {
        match self {
            ManifestParsingError::Error(e) => e.span(),
            _ => None,
        }
    }
    fn message(&self) -> String {
        match self {
            ManifestParsingError::Error(e) => e.message().to_owned(),
            _ => self.to_string(),
        }
    }
}

/// Describes the contents of a parsed global project manifest.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ParsedManifest {
    /// The version of the manifest
    version: ManifestVersion,
    /// The environments the project can create.
    pub(crate) envs: IndexMap<EnvironmentName, ParsedEnvironment>,
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub(crate) fn from_toml_str(source: &str) -> Result<Self, ManifestParsingError> {
        // If it fails only try to get version from the file to see if we can create a better error based on that.
        match toml_edit::de::from_str::<Self>(source) {
            Ok(toml) => Ok(toml),
            Err(e) => {
                // Define a struct that only includes the `version` field.
                #[derive(Deserialize)]
                struct VersionOnly {
                    version: ManifestVersion,
                }

                // Attempt to deserialize the TOML string into `VersionOnly`.
                match toml_edit::de::from_str::<VersionOnly>(source) {
                    Ok(version_only) => {
                        let version = version_only.version.0;
                        // Check if the version is supported.
                        match version.cmp(&GLOBAL_MANIFEST_VERSION) {
                            Ordering::Greater => Err(ManifestParsingError::VersionTooHigh(
                                version,
                                TomlError::from(e),
                            )),
                            Ordering::Less => Err(ManifestParsingError::VersionTooLow(
                                version,
                                TomlError::from(e),
                            )),
                            // Version is supported, but there was another parsing error.
                            Ordering::Equal => Err(ManifestParsingError::Error(TomlError::from(e))),
                        }
                    }
                    Err(_) => {
                        // Could not extract the version; return the original error.
                        Err(ManifestParsingError::Error(TomlError::from(e)))
                    }
                }
            }
        }
    }
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
                channels,
                package,
                executable_name,
                exposed,
            } = data;
            let parsed_environment = envs.entry(env_name).or_default();
            parsed_environment.channels.extend(channels);
            parsed_environment.platform = platform;
            parsed_environment
                .dependencies
                .insert(package, PixiSpec::default());
            parsed_environment
                .exposed
                .insert(Mapping::new(exposed, executable_name));
        }

        Self {
            envs,
            version: ManifestVersion::default(),
        }
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
            /// The version of the manifest
            #[serde(default)]
            version: ManifestVersion,
            /// The environments the project can create.
            #[serde(default)]
            envs: IndexMap<EnvironmentName, ParsedEnvironment>,
        }

        let manifest = TomlManifest::deserialize(deserializer)?;

        // Check for duplicate keys in the exposed fields
        let mut exposed_names = IndexSet::new();
        let mut duplicates = IndexMap::new();
        for key in manifest
            .envs
            .values()
            .flat_map(|env| env.exposed.iter().map(|m| m.exposed_name()))
        {
            if !exposed_names.insert(key) {
                duplicates.entry(key).or_insert_with(Vec::new).push(key);
            }
        }
        if !duplicates.is_empty() {
            return Err(serde::de::Error::custom(format!(
                "Duplicated exposed names found: {}",
                duplicates
                    .keys()
                    .sorted()
                    .map(|exposed_name| exposed_name.fancy_display())
                    .join(", ")
            )));
        }

        Ok(Self {
            version: manifest.version,
            envs: manifest.envs,
        })
    }
}

/// Deserialize a map of exposed names to executable names.
fn deserialize_expose_mappings<'de, D>(deserializer: D) -> Result<IndexSet<Mapping>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: HashMap<ExposedName, String> = HashMap::deserialize(deserializer)?;
    Ok(map
        .into_iter()
        .map(|(exposed_name, executable_name)| Mapping::new(exposed_name, executable_name))
        .collect())
}

/// Custom serializer for a map of exposed names to executable names.
fn serialize_expose_mappings<S>(
    mappings: &IndexSet<Mapping>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = serializer.serialize_map(Some(mappings.len()))?;
    for mapping in mappings {
        map.serialize_entry(&mapping.exposed_name(), &mapping.executable_name())?;
    }
    map.end()
}

#[serde_as]
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ParsedEnvironment {
    #[serde_as(as = "IndexSet<pixi_manifest::toml::TomlPrioritizedChannel>")]
    pub channels: IndexSet<pixi_manifest::PrioritizedChannel>,
    // Platform used by the environment.
    pub platform: Option<Platform>,
    #[serde(default)]
    pub(crate) dependencies: UniquePackageMap,
    #[serde(
        default,
        deserialize_with = "deserialize_expose_mappings",
        serialize_with = "serialize_expose_mappings"
    )]
    pub(crate) exposed: IndexSet<Mapping>,
}

impl ParsedEnvironment {
    // Create empty parsed environment
    pub(crate) fn new(channels: impl IntoIterator<Item = PrioritizedChannel>) -> Self {
        Self {
            channels: channels.into_iter().collect(),
            ..Default::default()
        }
    }
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

    /// Returns the exposed name mappings associated with this environment.
    pub(crate) fn exposed(&self) -> &IndexSet<Mapping> {
        &self.exposed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub(crate) struct ExposedName(String);

impl fmt::Display for ExposedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FancyDisplay for ExposedName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::EXPOSED_NAME_STYLE.apply_to(&self.0)
    }
}

#[derive(Error, Diagnostic, Debug, PartialEq)]
pub(crate) enum ExposedNameError {
    #[error("'pixi' is not allowed as exposed name in the map")]
    PixiBinParseError,
}

impl FromStr for ExposedName {
    type Err = ExposedNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "pixi" {
            Err(ExposedNameError::PixiBinParseError)
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
            r#"[envs."python;3".dependencies]"#,
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

        fn remove_ansi_escape_sequences(input: &str) -> String {
            use regex::Regex;
            let re = Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").unwrap();
            re.replace_all(input, "").to_string()
        }
        // Because this error is using `fancy_display`, we need to remove the ANSI escape sequences
        // before comparing the error message. In CI an interactive tty is not available so the result
        // will be different when running it locally, it seems :shrug:
        //
        // Might be better for the error implement `Diagnostic`
        assert!(manifest.is_err());
        let err = format!("{}", manifest.unwrap_err());
        assert_snapshot!(remove_ansi_escape_sequences(&err));
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
        channels = ["https://prefix.dev/conda-forge"]
        # It will expose python3.10
        [envs.python3-10.dependencies]
        python = "3.10.*"

        [envs.python3-10.exposed]
        "python3.10" = "python"
        "#;
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }
}
