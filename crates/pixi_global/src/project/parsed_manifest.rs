use std::{cmp::Ordering, fmt, path::Path, str::FromStr};

use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::{Either, Itertools};
use miette::{Context, Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use pixi_consts::consts;
use pixi_manifest::{PrioritizedChannel, toml::TomlPlatform, utils::package_map::UniquePackageMap};
use pixi_spec::PixiSpec;
use pixi_toml::{TomlFromStr, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_with::serde_derive::Deserialize;
use thiserror::Error;
use toml_span::{DeserError, Deserialize, Value, de_helpers::TableHelper};

use super::{ExposedData, environment::EnvironmentName};
use crate::{Mapping, project::manifest::TomlMapping};

pub const GLOBAL_MANIFEST_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ManifestVersion(i64);

impl Default for ManifestVersion {
    fn default() -> Self {
        ManifestVersion(GLOBAL_MANIFEST_VERSION)
    }
}

impl From<ManifestVersion> for toml_edit::Item {
    fn from(version: ManifestVersion) -> Self {
        toml_edit::value(version.0)
    }
}

#[derive(Error, Debug, Clone)]
pub enum ManifestParsingError {
    #[error(transparent)]
    Error(#[from] toml_edit::TomlError),
    #[error(transparent)]
    TomlError(#[from] toml_span::Error),
    #[error(
        "The 'version' of the manifest is too low: '{0}', the supported version is '{GLOBAL_MANIFEST_VERSION}', please update the manifest"
    )]
    VersionTooLow(i64, #[source] toml_span::Error),
    #[error(
        "The 'version' of the manifest is too high: '{0}', the supported version is '{GLOBAL_MANIFEST_VERSION}', please update `pixi` to support the new manifest version"
    )]
    VersionTooHigh(i64, #[source] toml_span::Error),
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
            ManifestParsingError::TomlError(e) => Some(e.span.into()),
            _ => None,
        }
    }
    fn message(&self) -> String {
        match self {
            ManifestParsingError::Error(e) => e.message().to_owned(),
            ManifestParsingError::TomlError(err) => match &err.kind {
                toml_span::ErrorKind::UnexpectedKeys { expected, .. } => {
                    format!(
                        "Unexpected keys, expected only {}",
                        expected
                            .iter()
                            .format_with(", ", |key, f| f(&format_args!("'{}'", key)))
                    )
                }
                toml_span::ErrorKind::UnexpectedValue { expected, .. } => {
                    format!(
                        "Expected one of {}",
                        expected
                            .iter()
                            .format_with(", ", |key, f| f(&format_args!("'{}'", key)))
                    )
                }
                _ => err.to_string(),
            },
            _ => self.to_string(),
        }
    }
}

/// Describes the contents of a parsed global project manifest.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ParsedManifest {
    /// The version of the manifest
    version: ManifestVersion,
    /// The environments the project can create.
    pub envs: IndexMap<EnvironmentName, ParsedEnvironment>,
}

impl<'de> toml_span::Deserialize<'de> for ParsedManifest {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let version = th
            .optional("version")
            .map(ManifestVersion)
            .unwrap_or_default();
        let envs = th
            .optional::<TomlIndexMap<_, ParsedEnvironment>>("envs")
            .map(TomlIndexMap::into_inner)
            .unwrap_or_default();

        ensure_unique_exposed_names(value, &envs)?;
        ensure_unique_shortcut_names(value, &envs)?;

        th.finalize(None)?;

        Ok(Self { version, envs })
    }
}

fn ensure_unique_exposed_names(
    value: &mut Value<'_>,
    envs: &IndexMap<EnvironmentName, ParsedEnvironment>,
) -> Result<(), DeserError> {
    let mut exposed_names = IndexSet::new();
    let mut duplicates = IndexMap::new();
    for key in envs
        .values()
        .flat_map(|env| env.exposed.iter().map(|m| m.exposed_name()))
    {
        if !exposed_names.insert(key) {
            duplicates.entry(key).or_insert_with(Vec::new).push(key);
        }
    }
    if !duplicates.is_empty() {
        return Err(DeserError::from(toml_span::Error {
            kind: toml_span::ErrorKind::Custom(
                format!(
                    "Duplicated exposed names found: {}",
                    duplicates
                        .keys()
                        .sorted()
                        .map(|exposed_name| exposed_name.fancy_display())
                        .join(", ")
                )
                .into(),
            ),
            span: value.span,
            line_info: None,
        }));
    }
    Ok(())
}

fn ensure_unique_shortcut_names(
    value: &mut Value<'_>,
    envs: &IndexMap<EnvironmentName, ParsedEnvironment>,
) -> Result<(), DeserError> {
    let mut shortcut_names = IndexSet::new();
    let mut duplicates = IndexMap::new();
    for key in envs.values().flat_map(|env| env.shortcuts.iter().flatten()) {
        if !shortcut_names.insert(key) {
            duplicates.entry(key).or_insert_with(Vec::new).push(key);
        }
    }
    if !duplicates.is_empty() {
        return Err(DeserError::from(toml_span::Error {
            kind: toml_span::ErrorKind::Custom(
                format!(
                    "Duplicated shortcut names found: {}",
                    duplicates
                        .keys()
                        .sorted()
                        .map(|shortcut_name| console::style(shortcut_name.as_normalized()).green())
                        .join(", ")
                )
                .into(),
            ),
            span: value.span,
            line_info: None,
        }));
    }
    Ok(())
}

impl ParsedManifest {
    /// Parses a toml string into a project manifest.
    pub(crate) fn from_toml_str(source: &str) -> Result<Self, ManifestParsingError> {
        let mut toml_value = toml_span::parse(source)?;

        let version = toml_value
            .as_table()
            .and_then(|table| table.get("version"))
            .and_then(|version| version.as_integer());

        match ParsedManifest::deserialize(&mut toml_value) {
            Ok(manifest) => Ok(manifest),
            Err(e) => {
                let error = e
                    .errors
                    .into_iter()
                    .next()
                    .expect("there should be at least one error");
                if let Some(version) = version {
                    // Check if the version is supported.
                    match version.cmp(&GLOBAL_MANIFEST_VERSION) {
                        Ordering::Greater => {
                            Err(ManifestParsingError::VersionTooHigh(version, error))
                        }
                        Ordering::Less => Err(ManifestParsingError::VersionTooLow(version, error)),
                        // Version is supported, but there was another parsing error.
                        Ordering::Equal => Err(ManifestParsingError::TomlError(error)),
                    }
                } else {
                    Err(ManifestParsingError::TomlError(error))
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
                .specs
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

#[derive(Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ParsedEnvironment {
    pub channels: IndexSet<PrioritizedChannel>,
    /// Platform used by the environment.
    pub platform: Option<Platform>,
    pub dependencies: UniquePackageMap,
    #[serde(default, serialize_with = "serialize_expose_mappings")]
    pub exposed: IndexSet<Mapping>,
    pub shortcuts: Option<IndexSet<PackageName>>,
}

impl<'de> toml_span::Deserialize<'de> for ParsedEnvironment {
    fn deserialize(value: &mut toml_span::Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let channels = th
            .optional::<TomlIndexSet<PrioritizedChannel>>("channels")
            .map(TomlIndexSet::into_inner)
            .unwrap_or_default();
        let platform = th.optional::<TomlPlatform>("platform").map(Platform::from);
        let dependencies = th.optional("dependencies").unwrap_or_default();
        let exposed = th
            .optional::<TomlMapping>("exposed")
            .map(TomlMapping::into_inner)
            .unwrap_or_default();
        let shortcuts = th
            .optional_s::<TomlWith<_, TomlIndexSet<TomlFromStr<PackageName>>>>("shortcuts")
            .map(|s| s.value.into_inner());

        th.finalize(None)?;

        Ok(Self {
            channels,
            platform,
            dependencies,
            exposed,
            shortcuts,
        })
    }
}

impl ParsedEnvironment {
    // Create empty parsed environment
    pub(crate) fn new(channels: impl IntoIterator<Item = PrioritizedChannel>) -> Self {
        Self {
            channels: channels.into_iter().collect(),
            ..Default::default()
        }
    }

    /// Returns the channels associated with this environment.
    pub(crate) fn channels(&self) -> IndexSet<&NamedChannelOrUrl> {
        PrioritizedChannel::sort_channels_by_priority(&self.channels).collect()
    }

    /// Splits the dependencies into source and binary requirements.
    pub(crate) fn split_into_source_and_binary_requirements(
        &self,
    ) -> (UniquePackageMap, UniquePackageMap) {
        self.dependencies
            .specs
            .clone()
            .into_iter()
            .partition_map(
                |(name, constraint)| match constraint.into_source_or_binary() {
                    Either::Left(source) => Either::Left((name, source)),
                    Either::Right(binary) => Either::Right((name, binary)),
                },
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub struct ExposedName(String);

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
pub enum ExposedNameError {
    #[error(
        "'{0}' is not allowed as exposed name in the map",
        pixi_utils::executable_name()
    )]
    PixiBinParseError,
}

impl FromStr for ExposedName {
    type Err = ExposedNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == pixi_utils::executable_name() {
            Err(ExposedNameError::PixiBinParseError)
        } else {
            Ok(ExposedName(value.to_string()))
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for ExposedName {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(toml_span::de_helpers::parse(value)?)
    }
}

impl AsRef<str> for ExposedName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Represents an error that occurs when parsing an binary exposed name.
///
/// This error is returned when a string fails to be parsed as an environment
/// name.
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
        assert_snapshot!(
            examples
                .into_iter()
                .map(|example| ParsedManifest::from_toml_str(example)
                    .unwrap_err()
                    .to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
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
        // Because this error is using `fancy_display`, we need to remove the ANSI
        // escape sequences before comparing the error message. In CI an
        // interactive tty is not available so the result will be different when
        // running it locally, it seems :shrug:
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
        pixi-bin-name = "python"
        "#;

        // Replace the pixi-bin-name with the actual executable name that can be variable at runtime in tests
        let contents = contents.replace("pixi-bin-name", pixi_utils::executable_name());
        let manifest = ParsedManifest::from_toml_str(contents.as_str());

        assert!(manifest.is_err());
        // Replace back the executable name with "pixi" to satisfy the snapshot
        assert_snapshot!(
            manifest
                .unwrap_err()
                .to_string()
                .replace(pixi_utils::executable_name(), "pixi")
        );
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
