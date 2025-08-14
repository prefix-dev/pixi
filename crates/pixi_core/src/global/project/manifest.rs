use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use fancy_display::FancyDisplay;
use fs_err::tokio as tokio_fs;
use indexmap::IndexSet;
use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_consts::consts;
use pixi_manifest::{PrioritizedChannel, toml::TomlDocument};
use pixi_toml::TomlIndexMap;
use pixi_utils::{executable_from_path, strip_executable_extension};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, Platform};
use toml_edit::{DocumentMut, Item};
use toml_span::{DeserError, Value};

use super::{
    EnvironmentName, ExposedName,
    global_spec::NamedGlobalSpec,
    parsed_manifest::{ManifestParsingError, ManifestVersion, ParsedManifest},
};
use crate::global::project::ParsedEnvironment;

/// Handles the global project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`ParsedManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// Editable toml document
    pub document: TomlDocument,

    /// The parsed manifest
    pub parsed: ParsedManifest,
}

impl Manifest {
    /// Creates a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let manifest_path = dunce::canonicalize(path.as_ref()).into_diagnostic()?;
        let contents = fs_err::read_to_string(path.as_ref()).into_diagnostic()?;
        Self::from_str(manifest_path.as_ref(), contents)
    }

    /// Creates a new manifest from a string
    pub fn from_str(manifest_path: &Path, contents: impl Into<String>) -> miette::Result<Self> {
        let contents = contents.into();
        let parsed = ParsedManifest::from_toml_str(&contents);

        let (manifest, document) = match parsed.and_then(|manifest| {
            contents
                .parse::<DocumentMut>()
                .map(|doc| (manifest, doc))
                .map_err(ManifestParsingError::from)
        }) {
            Ok(result) => result,
            Err(e) => e.to_fancy(
                consts::GLOBAL_MANIFEST_DEFAULT_NAME,
                &contents,
                manifest_path,
            )?,
        };

        let manifest = Self {
            path: manifest_path.to_path_buf(),

            document: TomlDocument::new(document),
            parsed: manifest,
        };

        Ok(manifest)
    }

    /// Adds an environment to the manifest
    pub fn add_environment(
        &mut self,
        env_name: &EnvironmentName,
        channels: Option<Vec<NamedChannelOrUrl>>,
    ) -> miette::Result<()> {
        let channels = channels
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| Config::load_global().default_channels());

        // Update self.parsed
        if self.parsed.envs.get(env_name).is_some() {
            miette::bail!("Environment {} already exists.", env_name.fancy_display());
        }
        self.parsed.envs.insert(
            env_name.clone(),
            ParsedEnvironment::new(channels.clone().into_iter().map(PrioritizedChannel::from)),
        );

        // Update self.document
        let channels_array = self
            .document
            .get_or_insert_toml_array_mut(&format!("envs.{env_name}"), "channels")?;
        for channel in channels {
            channels_array.push(channel.as_str());
        }

        tracing::debug!(
            "Added environment {} to toml document",
            env_name.fancy_display()
        );
        Ok(())
    }

    /// Removes a specific environment from the manifest
    pub fn remove_environment(&mut self, env_name: &EnvironmentName) -> miette::Result<()> {
        // Update self.parsed
        self.parsed.envs.shift_remove(env_name).ok_or_else(|| {
            miette::miette!("Environment {} doesn't exist.", env_name.fancy_display())
        })?;

        // Update self.document
        self.document
            .get_or_insert_nested_table("envs")?
            .remove(env_name.as_str())
            .ok_or_else(|| {
                miette::miette!("Environment {} doesn't exist.", env_name.fancy_display())
            })?;

        tracing::debug!(
            "Removed environment {} from toml document",
            env_name.fancy_display()
        );
        Ok(())
    }

    /// Adds a dependency to the manifest
    pub fn add_dependency(
        &mut self,
        env_name: &EnvironmentName,
        named_spec: &NamedGlobalSpec,
    ) -> miette::Result<()> {
        let name = named_spec.name();
        let spec = named_spec.spec();

        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| {
                miette::miette!("Environment {} doesn't exist.", env_name.fancy_display())
            })?
            .dependencies
            .specs
            .insert(name.clone(), spec.clone());

        // Update self.document
        self.document.insert_into_inline_table(
            &format!("envs.{env_name}.dependencies"),
            name.as_normalized(),
            spec.to_toml_value(),
        )?;

        tracing::debug!(
            "Added dependency {}={} to toml document for environment {}",
            name.as_normalized(),
            spec.to_toml_value().to_string(),
            env_name.fancy_display()
        );
        Ok(())
    }

    /// Removes a dependency from the manifest
    pub fn remove_dependency(
        &mut self,
        env_name: &EnvironmentName,
        name: &PackageName,
    ) -> miette::Result<PackageName> {
        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| {
                miette::miette!("Environment {} doesn't exist.", env_name.fancy_display())
            })?
            .dependencies
            .specs
            .swap_remove(name)
            .ok_or(miette::miette!(
                "Dependency {} not found in {}",
                console::style(name.as_normalized()).green(),
                env_name.fancy_display()
            ))?;

        // Update self.document
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.dependencies"))?
            .remove(name.as_normalized());

        tracing::debug!(
            "Removed dependency {} to toml document for environment {}",
            console::style(name.as_normalized()).green(),
            env_name.fancy_display()
        );
        Ok(name.clone())
    }

    /// Sets the platform of a specific environment in the manifest
    pub fn set_platform(
        &mut self,
        env_name: &EnvironmentName,
        platform: Platform,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist", env_name.fancy_display());
        }

        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| {
                miette::miette!("Can't find environment {} yet", env_name.fancy_display())
            })?
            .platform
            .replace(platform);

        // Update self.document
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}"))?
            .insert(
                "platform",
                Item::Value(toml_edit::Value::from(platform.to_string())),
            );

        tracing::debug!(
            "Set platform {} for environment {} in toml document",
            platform,
            env_name
        );
        Ok(())
    }

    #[allow(dead_code)]
    /// Adds a channel to the manifest
    pub fn add_channel(
        &mut self,
        env_name: &EnvironmentName,
        channel: &NamedChannelOrUrl,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist", env_name.fancy_display());
        }

        // Update self.parsed
        let env = self
            .parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("This should be impossible"))?;
        env.channels
            .insert(PrioritizedChannel::from(channel.clone()));

        // Update self.document
        let channels_array = self
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}"))?
            .entry("channels")
            .or_insert_with(|| toml_edit::Item::Value(toml_edit::Value::Array(Default::default())))
            .as_array_mut()
            .ok_or_else(|| miette::miette!("Expected an array for channels"))?;

        // Convert existing TOML array to a IndexSet to ensure uniqueness
        let mut existing_channels: IndexSet<String> = channels_array
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect();

        // Add the new channel to the HashSet
        existing_channels.insert(channel.to_string());

        // Reinsert unique channels
        *channels_array = existing_channels.iter().collect();

        tracing::debug!("Added channel {channel} for environment {env_name} in toml document");
        Ok(())
    }

    /// Matches an exposed name to its corresponding environment name.
    pub fn match_exposed_name_to_environment(
        &self,
        exposed_name: &ExposedName,
    ) -> miette::Result<EnvironmentName> {
        for (env_name, env) in &self.parsed.envs {
            for mapping in &env.exposed {
                if mapping.exposed_name == *exposed_name {
                    return Ok(env_name.clone());
                }
            }
        }
        Err(miette::miette!(
            "Exposed name {} not found in any environment",
            exposed_name.fancy_display()
        ))
    }

    /// Checks if an exposed name already exists in other environments
    pub fn exposed_name_already_exists_in_other_envs(
        &self,
        env_name: &EnvironmentName,
        exposed_name: &ExposedName,
    ) -> bool {
        self.parsed
            .envs
            .iter()
            .filter_map(|(name, env)| if name != env_name { Some(env) } else { None })
            .flat_map(|env| env.exposed.iter())
            .any(|mapping| mapping.exposed_name == *exposed_name)
    }

    /// Adds exposed mapping to the manifest
    pub fn add_exposed_mapping(
        &mut self,
        env_name: &EnvironmentName,
        mapping: &Mapping,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist", env_name.fancy_display());
        }

        // Ensure exposed name is unique
        if self.exposed_name_already_exists_in_other_envs(env_name, &mapping.exposed_name) {
            miette::bail!(
                "Exposed name {} already exists",
                mapping.exposed_name.fancy_display()
            );
        }

        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("This should be impossible"))?
            .exposed
            .insert(mapping.clone());

        // Update self.document
        self.document.insert_into_inline_table(
            &format!("envs.{env_name}.exposed"),
            mapping.exposed_name.as_ref(),
            toml_edit::Value::from(mapping.executable_relname.clone()),
        )?;

        tracing::debug!("Added exposed mapping {mapping} to toml document");
        Ok(())
    }

    /// Removes exposed mapping from the manifest
    pub fn remove_exposed_name(
        &mut self,
        env_name: &EnvironmentName,
        exposed_name: &ExposedName,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist", env_name.fancy_display());
        }
        let environment = self
            .parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("[envs.{env_name}] needs to exist"))?;

        // Remove exposed_name from parsed environment
        environment
            .exposed
            .retain(|map| map.exposed_name() != exposed_name);

        // Remove from the document
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .remove(exposed_name.as_ref())
            .ok_or_else(|| miette::miette!("The exposed name {exposed_name} doesn't exist"))?;

        tracing::debug!("Removed exposed mapping {exposed_name} from toml document");
        Ok(())
    }

    /// Removes all exposed mappings for a specific environment
    pub fn remove_all_exposed_mappings(
        &mut self,
        env_name: &EnvironmentName,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        let env = self.parsed.envs.get_mut(env_name).ok_or_else(|| {
            miette::miette!("Environment {} doesn't exist", env_name.fancy_display())
        })?;

        // Clear the exposed mappings
        env.exposed.clear();

        // Update self.document
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}"))?
            .remove("exposed");

        tracing::debug!(
            "Removed all exposed mappings for environment {} in toml document",
            env_name.fancy_display()
        );
        Ok(())
    }

    /// Checks if an exposed name already exists in other environments
    pub fn shortcut_already_exists_in_other_envs(
        &self,
        env_name: &EnvironmentName,
        shortcut: &PackageName,
    ) -> bool {
        self.parsed
            .envs
            .iter()
            .filter_map(|(name, env)| if name != env_name { Some(env) } else { None })
            .flat_map(|env| env.shortcuts.iter().flat_map(|s| s.iter()))
            .any(|s| s == shortcut)
    }

    /// Adds shortcut to the manifest
    pub fn add_shortcut(
        &mut self,
        env_name: &EnvironmentName,
        shortcut: &PackageName,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist", env_name.fancy_display());
        }

        // Ensure shortcut is unique
        if self.shortcut_already_exists_in_other_envs(env_name, shortcut) {
            miette::bail!(
                "Shortcut {} already exists",
                console::style(shortcut.as_normalized()).green()
            );
        }

        // Update self.parsed
        let env = self
            .parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("This should be impossible"))?;
        env.shortcuts
            .get_or_insert_default()
            .insert(shortcut.clone());

        // Update self.document
        let shortcuts_array = self
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}"))?
            .entry("shortcuts")
            .or_insert_with(|| toml_edit::Item::Value(toml_edit::Value::Array(Default::default())))
            .as_array_mut()
            .ok_or_else(|| miette::miette!("Expected an array for shortcuts"))?;

        // Convert existing TOML array to a IndexSet to ensure uniqueness
        let mut existing_shortcuts: IndexSet<String> = shortcuts_array
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect();

        // Add the new shortcut to the HashSet
        existing_shortcuts.insert(shortcut.as_normalized().to_string());

        // Reinsert unique shortcuts
        *shortcuts_array = existing_shortcuts.iter().collect();

        tracing::debug!(
            "Added shortcut {} for environment {} in toml document",
            console::style(shortcut.as_normalized()).green(),
            env_name.fancy_display()
        );
        Ok(())
    }

    /// Removes shortcut from the manifest of any environment
    pub fn remove_shortcut(
        &mut self,
        shortcut: &PackageName,
        env_name: &EnvironmentName,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            miette::bail!("Environment {} doesn't exist", env_name.fancy_display());
        }
        let environment = self
            .parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("[envs.{env_name}] needs to exist"))?;

        // Remove shortcut from parsed environment
        if let Some(shortcuts) = environment.shortcuts.as_mut() {
            if !shortcuts.contains(shortcut) {
                miette::bail!("The shortcut {} doesn't exist", shortcut.as_normalized());
            }

            shortcuts.swap_remove(shortcut);
            tracing::debug!(
                "Removed shortcut '{}' from toml document",
                shortcut.as_normalized()
            );
        }

        // Remove from the document
        let env_key = format!("envs.{env_name}");
        let shortcuts_array = self
            .document
            .get_mut_toml_array(&env_key, "shortcuts")?
            .ok_or_else(|| miette::miette!("No shortcuts found for environment {}", env_name))?;

        let shortcut_str = shortcut.as_normalized();
        // First find the index without holding onto the iterator
        let maybe_index = shortcuts_array
            .iter()
            .position(|item| item.as_str() == Some(shortcut_str));

        if let Some(index) = maybe_index {
            shortcuts_array.remove(index);
            tracing::debug!("Removed shortcut '{}' from toml document", shortcut_str);
        } else {
            return Err(miette::miette!(
                "The shortcut '{}' doesn't exist",
                shortcut_str
            ));
        }

        Ok(())
    }

    /// Saves the manifest to the file system
    pub async fn save(&self) -> miette::Result<()> {
        let contents = {
            // Ensure that version is always set when saving
            let mut document = self.document.clone();
            document.get_or_insert("version", ManifestVersion::default().into());
            document.to_string()
        };

        tokio_fs::write(&self.path, contents)
            .await
            .into_diagnostic()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Mapping {
    exposed_name: ExposedName,
    // The executable_relname is a executable name possibly with a parts of a path in it to match
    // on. e.g. `dotnet/dotnet` will find `$PREFIX/lib/dotnet/dotnet`
    executable_relname: String,
}

pub(super) struct TomlMapping(IndexSet<Mapping>);

impl TomlMapping {
    pub fn into_inner(self) -> IndexSet<Mapping> {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlMapping {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(Self(
            TomlIndexMap::deserialize(value)?
                .into_inner()
                .into_iter()
                .map(|(key, value)| Mapping::new(key, value))
                .collect(),
        ))
    }
}

impl Mapping {
    pub fn new(exposed_name: ExposedName, executable_relname: String) -> Self {
        Self {
            exposed_name,
            executable_relname: strip_executable_extension(executable_relname),
        }
    }

    pub fn exposed_name(&self) -> &ExposedName {
        &self.exposed_name
    }

    pub fn executable_relname(&self) -> &str {
        &self.executable_relname
    }

    // Splitting the executable_relname by the last '/' and taking the last part
    // e.g. 'nested/test_executable' -> 'test_executable'
    pub fn executable_name(&self) -> &str {
        Path::new(&self.executable_relname)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.executable_relname)
    }
}

impl fmt::Display for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.exposed_name, self.executable_relname)
    }
}

impl FromStr for Mapping {
    type Err = miette::Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        // If we can't parse exposed_name=executable_relname, assume input=input
        let (exposed_name, executable_relname) = input.split_once('=').unwrap_or((input, input));

        // Make sure we expose only the executable name, even with nested paths.
        // e.g. lib/bin/exec.exe -> exec
        let exposed_name = executable_from_path(Path::new(exposed_name));
        let exposed_name = ExposedName::from_str(exposed_name.as_str())?;

        Ok(Mapping::new(exposed_name, executable_relname.to_string()))
    }
}

/// Describes which executables should be (additionally) exposed
/// Only executables by direct dependencies are taken into account
#[derive(Default)]
pub enum ExposedType {
    #[default]
    All,
    Nothing,
    Ignore(Vec<PackageName>),
    Mappings(Vec<Mapping>),
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use indexmap::IndexSet;
    use insta::assert_snapshot;
    use itertools::Itertools;
    use pixi_consts::consts::DEFAULT_CHANNELS;
    use rattler_conda_types::ChannelConfig;

    use super::*;

    #[test]
    fn test_mapping_executable_names() {
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let executable_name = "test_executable".to_string();
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        assert_eq!("test_executable", mapping.executable_name());
        assert_eq!("test_executable", mapping.executable_relname());

        let executable_name = "nested/test_executable".to_string();
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        assert_eq!("test_executable", mapping.executable_name());
        assert_eq!("nested/test_executable", mapping.executable_relname());

        let executable_name: String;
        let expected_exe_relname: &str;
        #[cfg(windows)]
        {
            executable_name = "nested\\test_executable.exe".to_string();
            expected_exe_relname = "nested\\test_executable";
        }
        #[cfg(unix)]
        {
            executable_name = "nested/test_executable.sh".to_string();
            expected_exe_relname = "nested/test_executable";
        }
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        assert_eq!("test_executable", mapping.executable_name());
        assert_eq!(expected_exe_relname, mapping.executable_relname());
    }

    #[test]
    fn test_add_exposed_mapping_new_env() {
        let mut manifest = Manifest::default();
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let executable_name = "test_executable".to_string();
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        manifest.add_environment(&env_name, None).unwrap();

        let result = manifest.add_exposed_mapping(&env_name, &mapping);
        assert!(result.is_ok());

        let expected_value = "test_executable";

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{}.exposed", env_name))
            .unwrap()
            .get(exposed_name.as_ref())
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(expected_value, actual_value);

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .exposed
            .iter()
            .find(|map| map.exposed_name() == &exposed_name)
            .unwrap()
            .executable_relname();
        assert_eq!(expected_value, actual_value)
    }

    #[test]
    fn test_add_exposed_mapping_existing_env() {
        let mut manifest = Manifest::default();
        let exposed_name1 = ExposedName::from_str("test_exposed1").unwrap();
        let executable_relname1 = "test_executable1".to_string();
        let mapping1 = Mapping::new(exposed_name1.clone(), executable_relname1);
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        manifest.add_environment(&env_name, None).unwrap();

        manifest.add_exposed_mapping(&env_name, &mapping1).unwrap();

        let exposed_name2 = ExposedName::from_str("test_exposed2").unwrap();
        let executable_relname2 = "nested/test_executable2".to_string();
        let mapping2 = Mapping::new(exposed_name2.clone(), executable_relname2);
        let result = manifest.add_exposed_mapping(&env_name, &mapping2);
        assert!(result.is_ok());

        // Check document for executable1
        let expected_value1 = "test_executable1";
        let actual_value1 = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))
            .unwrap()
            .get(exposed_name1.as_ref())
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(expected_value1, actual_value1);

        // Check parsed for executable1
        let actual_value1 = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .exposed
            .iter()
            .find(|map| map.exposed_name() == &exposed_name1)
            .unwrap()
            .executable_relname();
        assert_eq!(expected_value1, actual_value1);

        // Check document for executable2
        let expected_value2 = "nested/test_executable2";
        let actual_value2 = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))
            .unwrap()
            .get(exposed_name2.as_ref())
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(expected_value2, actual_value2);

        // Check parsed for executable2
        let actual_value2 = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .exposed
            .iter()
            .find(|map| map.exposed_name() == &exposed_name2)
            .unwrap()
            .executable_relname();
        assert_eq!(expected_value2, actual_value2);
    }

    #[test]
    fn test_remove_exposed_mapping() {
        let mut manifest = Manifest::default();
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let executable_name = "test_executable".to_string();
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        // Add environment
        manifest.add_environment(&env_name, None).unwrap();

        // Add and remove mapping again
        manifest.add_exposed_mapping(&env_name, &mapping).unwrap();
        manifest
            .remove_exposed_name(&env_name, &exposed_name)
            .unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))
            .unwrap()
            .get(exposed_name.as_ref());
        assert!(actual_value.is_none());

        // Check parsed
        assert!(
            !manifest
                .parsed
                .envs
                .get(&env_name)
                .unwrap()
                .exposed
                .iter()
                .any(|map| map.exposed_name() == &exposed_name)
        );
    }

    #[test]
    fn test_remove_exposed_mapping_nonexistent() {
        let mut manifest = Manifest::default();
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        // Removing an exposed name that doesn't exist should return an error
        let result = manifest.remove_exposed_name(&env_name, &exposed_name);
        assert!(result.is_err())
    }

    #[test]
    fn test_add_environment_default_channel() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        // Add environment
        manifest.add_environment(&env_name, None).unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table("envs")
            .unwrap()
            .get(env_name.as_str());
        assert!(actual_value.is_some());

        // Check parsed
        let env = manifest.parsed.envs.get(&env_name).unwrap();

        // Check channels
        let expected_channels = Config::load_global()
            .default_channels()
            .into_iter()
            .map(From::from)
            .collect::<IndexSet<_>>();
        let actual_channels = env.channels.clone();
        assert_eq!(expected_channels, actual_channels);
    }

    #[test]
    fn test_add_environment_given_channel() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        let channels = Vec::from([
            NamedChannelOrUrl::from_str("test-channel-1").unwrap(),
            NamedChannelOrUrl::from_str("test-channel-2").unwrap(),
        ]);

        // Add environment
        manifest
            .add_environment(&env_name, Some(channels.clone()))
            .unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table("envs")
            .unwrap()
            .get(env_name.as_str());
        assert!(actual_value.is_some());

        // Check parsed
        let env = manifest.parsed.envs.get(&env_name).unwrap();

        // Check channels
        let expected_channels = channels
            .into_iter()
            .map(From::from)
            .collect::<IndexSet<_>>();
        let actual_channels = env.channels.clone();
        assert_eq!(expected_channels, actual_channels);
    }

    #[test]
    fn test_remove_environment() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        // Add environment
        manifest.add_environment(&env_name, None).unwrap();

        // Remove environment
        manifest.remove_environment(&env_name).unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table("envs")
            .unwrap()
            .get(env_name.as_str());
        assert!(actual_value.is_none());

        // Check parsed
        let actual_value = manifest.parsed.envs.get(&env_name);
        assert!(actual_value.is_none());
    }

    #[test]
    fn test_remove_non_existent_environment() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("non-existent-env").unwrap();

        // Remove non-existent environment
        let result = manifest.remove_environment(&env_name);

        // This should fail
        assert!(result.is_err());
    }

    #[test]
    fn test_add_dependency() {
        let mut manifest = Manifest::default();
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        let named_global_spec =
            NamedGlobalSpec::try_from_str("pythonic ==3.15.0", &channel_config).unwrap();

        // Add environment
        manifest
            .add_environment(
                &env_name,
                Some(
                    DEFAULT_CHANNELS
                        .iter()
                        .map(|name| NamedChannelOrUrl::Name(name.to_string()))
                        .collect(),
                ),
            )
            .unwrap();

        // Add dependency
        manifest
            .add_dependency(&env_name, &named_global_spec)
            .unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.dependencies"))
            .unwrap()
            .get(named_global_spec.name().as_normalized());
        assert!(actual_value.is_some());
        assert_eq!(
            actual_value.unwrap().to_string().replace('"', ""),
            named_global_spec
                .spec()
                .as_version_spec()
                .unwrap()
                .to_string()
        );

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .dependencies
            .specs
            .get(named_global_spec.name().as_normalized())
            .unwrap()
            .clone();
        assert_eq!(actual_value, *named_global_spec.spec());

        // Add another dependency
        let build_match_spec = NamedGlobalSpec::try_from_str(
            "python [version='==3.11.0', build=he550d4f_1_cpython]",
            &channel_config,
        )
        .unwrap();
        manifest
            .add_dependency(&env_name, &build_match_spec)
            .unwrap();
        let any_spec = NamedGlobalSpec::try_from_str("any-spec", &channel_config).unwrap();
        manifest.add_dependency(&env_name, &any_spec).unwrap();

        assert_snapshot!(manifest.document.to_string());
    }

    #[test]
    fn test_add_existing_dependency() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let spec = NamedGlobalSpec::try_from_str("pythonic ==3.15.0", &channel_config).unwrap();

        // Add environment
        manifest.add_environment(&env_name, None).unwrap();

        // Add dependency
        manifest.add_dependency(&env_name, &spec).unwrap();

        // Add the same dependency again, with a new match_spec
        let new_spec = NamedGlobalSpec::try_from_str("pythonic==3.18.0", &channel_config).unwrap();
        manifest.add_dependency(&env_name, &new_spec).unwrap();

        // Check document
        let name = spec.name();
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.dependencies"))
            .unwrap()
            .get(name.as_normalized());
        assert!(actual_value.is_some());
        assert_eq!(
            actual_value.unwrap().to_string().replace('"', ""),
            "==3.18.0"
        );

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .dependencies
            .specs
            .get(name)
            .unwrap()
            .clone();
        assert_eq!(
            actual_value.into_version().unwrap().to_string(),
            "==3.18.0".to_string()
        );
    }

    #[test]
    fn test_set_platform() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        let platform = Platform::LinuxRiscv64;

        // Add environment
        manifest.add_environment(&env_name, None).unwrap();

        // Set platform
        manifest.set_platform(&env_name, platform).unwrap();

        // Check document
        let actual_platform = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}"))
            .unwrap()
            .get("platform")
            .unwrap();
        assert_eq!(actual_platform.as_str().unwrap(), platform.as_str());

        // Check parsed
        let actual_platform = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .platform
            .unwrap();
        assert_eq!(actual_platform, platform);
    }

    #[test]
    fn test_add_channel() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        let channel = NamedChannelOrUrl::from_str("test-channel").unwrap();
        let mut channels = Config::load_global().default_channels();
        channels.push(channel.clone());

        // Add environment
        manifest.add_environment(&env_name, None).unwrap();

        // Add channel
        manifest.add_channel(&env_name, &channel).unwrap();

        // Check document
        let actual_channels = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}"))
            .unwrap()
            .get("channels")
            .unwrap()
            .as_array()
            .unwrap()
            .into_iter()
            .filter_map(|v| v.as_str())
            .collect_vec();
        let expected_channels = channels.iter().map(|c| c.as_str()).collect_vec();
        assert_eq!(actual_channels, expected_channels);

        // Check parsed
        let actual_channels = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .channels
            .clone();
        let expected_channels: IndexSet<PrioritizedChannel> =
            channels.into_iter().map(From::from).collect();
        assert_eq!(actual_channels, expected_channels);
    }

    #[test]
    fn test_remove_dependency() {
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        let name = PackageName::from_str("pytest").unwrap();

        let mut manifest = Manifest::from_str(
            Path::new("global.toml"),
            r#"
[envs.test-env]
channels = ["test-channel"]
dependencies = { "python" = "*", pytest = "*"}
"#,
        )
        .unwrap();

        // Remove dependency
        manifest.remove_dependency(&env_name, &name).unwrap();

        // Check document
        assert!(
            !manifest
                .document
                .to_string()
                .contains(name.clone().as_normalized())
        );

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .dependencies
            .specs
            .get(&name);
        assert!(actual_value.is_none());

        assert_snapshot!(manifest.document.to_string());
    }
}
