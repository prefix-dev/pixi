use std::{cmp::Ordering, fmt, path::Path, str::FromStr};

use console::StyledObject;
use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{Context, Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use pixi_consts::consts;
use pixi_manifest::{toml::TomlPlatform, utils::package_map::UniquePackageMap, PrioritizedChannel};
use pixi_spec::PixiSpec;
use pixi_toml::{TomlIndexMap, TomlIndexSet};
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

        // Check for duplicate keys in the exposed fields
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

        th.finalize(None)?;

        Ok(Self { version, envs })
    }
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

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ParsedEnvironment {
    /// The channels to use for this environment.
    #[serde(default, skip_serializing_if = "IndexSet::is_empty")]
    pub channels: IndexSet<PrioritizedChannel>,

    /// Platform used by the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,

    #[serde(default, skip_serializing_if = "UniquePackageMap::is_empty")]
    pub(crate) dependencies: UniquePackageMap,

    #[serde(
        default,
        serialize_with = "serialize_expose_mappings",
        skip_serializing_if = "IndexSet::is_empty"
    )]
    pub(crate) exposed: IndexSet<Mapping>,

    /// Whether to install the menuitems for the environment, using the Menuinst technology
    /// True means we install the item for the package of the toplevel dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) menu_install: Option<bool>,
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
        let menu_install = th.optional("menu-install");

        th.finalize(None)?;

        Ok(Self {
            channels,
            platform,
            dependencies,
            exposed,
            menu_install,
        })
    }
}

impl From<&ParsedEnvironment> for toml_edit::Value {
    /// Converts this instance into a [`toml_edit::Value`].
    fn from(env: &ParsedEnvironment) -> Self {
        ::serde::Serialize::serialize(env, toml_edit::ser::ValueSerializer::new())
            .expect("conversion to toml cannot fail")
    }
}

impl ParsedEnvironment {
    /// Creates a minimal environment based on the channels and dependencies provided.
    pub fn new<C>(channels: C, dependencies: IndexMap<PackageName, PixiSpec>) -> Self
    where
        C: IntoIterator,
        C::Item: Into<PrioritizedChannel>,
    {
        Self {
            channels: channels.into_iter().map(Into::into).collect(),
            dependencies: UniquePackageMap {
                specs: dependencies,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Sets the platform for this environment
    pub fn set_platform(&mut self, platform: Platform) {
        self.platform = Some(platform)
    }

    /// Sets the menu install flag for this environment
    pub fn set_menu_install(&mut self, menu_install: bool) {
        self.menu_install = Some(menu_install)
    }

    /// Returns the platform associated with this platform, `None` means current
    /// platform
    pub(crate) fn platform(&self) -> Option<Platform> {
        self.platform
    }

    /// Returns the channels associated with this environment.
    pub(crate) fn channels(&self) -> IndexSet<&NamedChannelOrUrl> {
        PrioritizedChannel::sort_channels_by_priority(&self.channels).collect()
    }

    /// Returns the dependencies associated with this environment.
    pub(crate) fn dependencies(&self) -> &IndexMap<PackageName, PixiSpec> {
        &self.dependencies.specs
    }

    /// Returns the exposed name mappings associated with this environment.
    pub(crate) fn exposed(&self) -> &IndexSet<Mapping> {
        &self.exposed
    }

    /// Returns true if the menu items should be installed for this environment
    pub(crate) fn menu_install(&self) -> bool {
        self.menu_install.unwrap_or(true)
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

/// Represents an error that occurs when parsing an binary exposed name.
///
/// This error is returned when a string fails to be parsed as an environment
/// name.
#[derive(Debug, Clone, Error, Diagnostic, PartialEq)]
#[error("pixi is not allowed as exposed name in the map")]
pub struct ParseExposedKeyError {}

#[cfg(test)]
mod tests {
    use super::ParsedManifest;
    use crate::global::project::ParsedEnvironment;
    use indexmap::IndexMap;
    use insta::assert_snapshot;
    use itertools::Itertools;
    use pixi_manifest::PrioritizedChannel;
    use pixi_spec::PixiSpec;
    use rattler_conda_types::{
        NamedChannelOrUrl, PackageName, ParseStrictness, Platform, VersionSpec,
    };
    use std::str::FromStr;

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
        assert_snapshot!(manifest
            .unwrap_err()
            .to_string()
            .replace(pixi_utils::executable_name(), "pixi"));
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

    #[test]
    fn test_menu_install_deserialization() {
        let contents = r#"
        # The name of the environment is `python`
        [envs.spyder]
        channels = ["conda-forge"]
        dependencies = { spyder = "*" }
        exposed = { spyder = "spyder" }
        menu-install = true
        "#;
        let _manifest = ParsedManifest::from_toml_str(contents).unwrap();
    }

    #[test]
    fn test_serialization() {
        let channels = vec!["bioconda", "https://prefix.dev/conda-forge"]
            .into_iter()
            .map(|channel| PrioritizedChannel::from(NamedChannelOrUrl::from_str(channel).unwrap()))
            .collect_vec();

        let dependencies: IndexMap<PackageName, PixiSpec> =
            vec![("python", "==3.13.0"), ("pixi", "*")]
                .into_iter()
                .map(|(package, version)| {
                    (
                        PackageName::from_str(package).unwrap(),
                        PixiSpec::from(
                            VersionSpec::from_str(version, ParseStrictness::Strict).unwrap(),
                        ),
                    )
                })
                .collect();

        let mut parsed_env = ParsedEnvironment::new(channels, dependencies);
        parsed_env.set_platform(Platform::Win64);
        parsed_env.set_menu_install(true);

        assert_snapshot!(::serde::Serialize::serialize(&parsed_env, toml_edit::ser::ValueSerializer::new()).unwrap(),
            @r#"{ channels = ["bioconda", "https://prefix.dev/conda-forge"], platform = "win-64", dependencies = { python = "==3.13.0", pixi = "*" }, menu-install = true }"#);
    }
}
