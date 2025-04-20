use std::{cmp::Ordering, fmt, path::Path, str::FromStr};

use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{Context, Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use pixi_consts::consts;
use pixi_manifest::{toml::TomlPlatform, utils::package_map::UniquePackageMap, PrioritizedChannel};
use pixi_spec::PixiSpec;
use pixi_toml::{TomlFromStr, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use serde::{ser::SerializeMap, Serialize, Serializer};
use serde_with::serde_derive::Deserialize;
use thiserror::Error;
use toml_span::{de_helpers::TableHelper, DeserError, Deserialize, Value};

use super::{environment::EnvironmentName, ExposedData};
use crate::global::{project::manifest::TomlMapping, Mapping};

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
    #[error("The 'version' of the manifest is too low: '{0}', the supported version is '{GLOBAL_MANIFEST_VERSION}', please update the manifest"
    )]
    VersionTooLow(i64, #[source] toml_span::Error),
    #[error("The 'version' of the manifest is too high: '{0}', the supported version is '{GLOBAL_MANIFEST_VERSION}', please update `pixi` to support the new manifest version"
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
    pub(crate) envs: IndexMap<EnvironmentName, ParsedEnvironment>,
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
pub(crate) struct ParsedEnvironment {
    pub(crate) channels: IndexSet<PrioritizedChannel>,
    /// Platform used by the environment.
    pub(crate) platform: Option<Platform>,
    pub(crate) dependencies: UniquePackageMap,
    #[serde(default, serialize_with = "serialize_expose_mappings")]
    pub(crate) exposed: IndexSet<Mapping>,
    pub(crate) shortcuts: Option<IndexSet<PackageName>>,
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

/// Error that occurs when trying to use a reserved name as an exposed binary name.
#[derive(Error, Debug, Clone)]
pub(crate) enum ExposedNameError {
    /// Error when attempting to use the package manager's name as an exposed binary name
    #[error("The name '{0}' is reserved and cannot be used as an exposed name")]
    #[diagnostic(
        help("The name 'pixi' is reserved for the package manager itself. Please choose a different name for your exposed binary."),
        code("pixi::exposed_name::reserved_name")
    )]
    PixiBinParseError(String),

    /// Error when the name contains invalid characters or formatting
    #[error("The name '{0}' contains invalid characters or formatting")]
    #[diagnostic(
        help("Exposed binary names should only contain alphanumeric characters, hyphens, and underscores. They cannot start with a hyphen or underscore."),
        code("pixi::exposed_name::invalid_format")
    )]
    InvalidFormatError(String),
}

impl FromStr for ExposedName {
    type Err = ExposedNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Check for reserved name
        if value == pixi_utils::executable_name() {
            return Err(ExposedNameError::PixiBinParseError(value.to_string()));
        }

        // Validate name format
        if !is_valid_exposed_name(value) {
            return Err(ExposedNameError::InvalidFormatError(value.to_string()));
        }

        Ok(ExposedName(value.to_string()))
    }
}

/// Validates if a name is suitable for use as an exposed binary name.
///
/// Rules:
/// - Must not be empty
/// - Must start with an alphanumeric character
/// - Can only contain alphanumeric characters, hyphens, and underscores
/// - Cannot be longer than 255 characters
fn is_valid_exposed_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }

    // Check if the first character is alphanumeric
    if !name.chars().next().map_or(false, |c| c.is_alphanumeric()) {
        return false;
    }

    // Check if all characters are valid
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
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

/// Error that occurs when parsing an exposed name that is reserved.
///
/// This error is returned when a string fails to be parsed as an exposed name
/// because it matches a reserved name (like 'pixi'). The error includes
/// diagnostic information to help users understand why the name is reserved
/// and what they should do instead.
#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("The name '{0}' is reserved and cannot be used as an exposed name")]
#[diagnostic(
    help("The name 'pixi' is reserved for the package manager itself. Please choose a different name for your exposed binary."),
    code("pixi::exposed_name::reserved_name")
)]
pub struct ParseExposedKeyError(String);

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
        let error_message = manifest.unwrap_err().to_string();
        let error_message = error_message.replace(pixi_utils::executable_name(), "pixi");
        assert_snapshot!(error_message);
    }

    #[test]
    fn test_reserved_name_error_message() {
        let contents = r#"
        [envs.test]
        channels = ["conda-forge"]
        [envs.test.dependencies]
        python = "*"
        [envs.test.exposed]
        pixi = "python"
        "#;

        // Replace the pixi-bin-name with the actual executable name that can be variable at runtime in tests
        let contents = contents.replace("pixi", pixi_utils::executable_name());
        let manifest = ParsedManifest::from_toml_str(contents.as_str());

        assert!(manifest.is_err());
        let error_message = manifest.unwrap_err().to_string();
        let error_message = error_message.replace(pixi_utils::executable_name(), "pixi");
        assert_snapshot!(error_message);
    }

    #[test]
    fn test_invalid_name_format() {
        let test_cases = vec![
            "",               // Empty string
            "-invalid",       // Starts with hyphen
            "_invalid",       // Starts with underscore
            "invalid@name",   // Contains invalid character
            "invalid name",   // Contains space
            &"a".repeat(256), // Too long
        ];

        for name in test_cases {
            let result = ExposedName::from_str(name);
            assert!(result.is_err(), "Expected error for name: {}", name);
            match result.unwrap_err() {
                ExposedNameError::InvalidFormatError(_) => (),
                _ => panic!("Expected InvalidFormatError for name: {}", name),
            }
        }
    }

    #[test]
    fn test_valid_name_format() {
        let test_cases = vec![
            "valid-name",
            "valid_name",
            "validName",
            "valid-name_123",
            "a",              // Single character
            &"a".repeat(255), // Maximum length
        ];

        for name in test_cases {
            let result = ExposedName::from_str(name);
            assert!(result.is_ok(), "Expected success for name: {}", name);
        }
    }

    #[test]
    fn test_case_sensitivity() {
        let name = "PIXI";
        let result = ExposedName::from_str(name);
        assert!(
            result.is_err(),
            "Expected error for uppercase name: {}",
            name
        );
        match result.unwrap_err() {
            ExposedNameError::PixiBinParseError(_) => (),
            _ => panic!("Expected PixiBinParseError for name: {}", name),
        }
    }
}
