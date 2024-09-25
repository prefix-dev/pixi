use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ahash::HashSet;
use fs_err as fs;
use fs_err::tokio as tokio_fs;
use miette::IntoDiagnostic;

use pixi_config::Config;
use pixi_manifest::{PrioritizedChannel, TomlError, TomlManifest};
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, PackageName, Platform, VersionSpec};
use toml_edit::{DocumentMut, Item};

use crate::global::project::ParsedEnvironment;

use super::parsed_manifest::ParsedManifest;
use super::{EnvironmentName, ExposedName, MANIFEST_DEFAULT_NAME};

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
    pub document: TomlManifest,

    /// The parsed manifest
    pub parsed: ParsedManifest,
}

impl Manifest {
    /// Creates a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let manifest_path = dunce::canonicalize(path.as_ref()).into_diagnostic()?;
        let contents = fs::read_to_string(path.as_ref()).into_diagnostic()?;
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
                .map_err(TomlError::from)
        }) {
            Ok(result) => result,
            Err(e) => e.to_fancy(MANIFEST_DEFAULT_NAME, &contents)?,
        };

        let manifest = Self {
            path: manifest_path.to_path_buf(),

            document: TomlManifest::new(document),
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
        self.parsed.envs.entry(env_name.clone()).or_insert_with(|| {
            ParsedEnvironment::new(channels.clone().into_iter().map(PrioritizedChannel::from))
        });

        // Update self.document
        let channels_array = self
            .document
            .get_or_insert_toml_array(&format!("envs.{env_name}"), "channels")?;
        for channel in channels {
            channels_array.push(channel.as_str());
        }

        tracing::debug!("Added environment {} to toml document", env_name);
        Ok(())
    }

    /// Removes a specific environment from the manifest
    pub fn remove_environment(&mut self, env_name: &EnvironmentName) -> miette::Result<()> {
        // Update self.parsed
        self.parsed.envs.shift_remove(env_name);

        // Update self.document
        self.document
            .get_or_insert_nested_table("envs")?
            .remove_entry(env_name.as_str());

        tracing::debug!("Removed environment {env_name} from toml document");
        Ok(())
    }

    /// Adds a dependency to the manifest
    pub fn add_dependency(
        &mut self,
        env_name: &EnvironmentName,
        dependency_name: &PackageName,
        spec: &MatchSpec,
    ) -> miette::Result<()> {
        let version = spec.version.clone().unwrap_or(VersionSpec::Any);
        let dependency_name_string = dependency_name.as_normalized();
        let version_string = version.to_string();

        if !self.parsed.envs.contains_key(env_name) {
            self.add_environment(env_name, None)?;
        }
        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("This should be impossible"))?
            .dependencies
            .insert(dependency_name.clone(), version.into());

        // Update self.document
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.dependencies"))?
            .insert(
                dependency_name_string,
                Item::Value(toml_edit::Value::from(version_string)),
            );

        tracing::debug!(
            "Added dependency {}={} to toml document for environment {}",
            dependency_name_string,
            spec,
            env_name
        );
        Ok(())
    }

    /// Sets the platform of a specific environment in the manifest
    pub fn set_platform(
        &mut self,
        env_name: &EnvironmentName,
        platform: Platform,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            self.add_environment(env_name, None)?;
        }

        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("This should be impossible"))?
            .platform = Some(platform);

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

    /// Adds a channel to the manifest
    pub fn add_channel(
        &mut self,
        env_name: &EnvironmentName,
        channel: &NamedChannelOrUrl,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            self.add_environment(env_name, None)?;
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

        // Convert existing TOML array to a HashSet to ensure uniqueness
        let mut existing_channels: HashSet<String> = channels_array
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect();

        // Add the new channel to the HashSet
        existing_channels.insert(channel.to_string());

        // Reinsert unique channels
        *channels_array = existing_channels.iter().collect();

        tracing::debug!("Added channel {channel} for environment {env_name} in toml document",);
        Ok(())
    }

    /// Adds exposed mapping to the manifest
    pub fn add_exposed_mapping(
        &mut self,
        env_name: &EnvironmentName,
        mapping: &Mapping,
    ) -> miette::Result<()> {
        // Ensure the environment exists
        if !self.parsed.envs.contains_key(env_name) {
            self.add_environment(env_name, None)?;
        }
        // Update self.parsed
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("This should be impossible"))?
            .exposed
            .insert(
                mapping.exposed_name.clone(),
                mapping.executable_name.clone(),
            );

        // Update self.document
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .insert(
                &mapping.exposed_name.to_string(),
                Item::Value(toml_edit::Value::from(mapping.executable_name.clone())),
            );

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
            self.add_environment(env_name, None)?;
        }
        self.parsed
            .envs
            .get_mut(env_name)
            .ok_or_else(|| miette::miette!("[envs.{env_name}] needs to exist"))?
            .exposed
            .shift_remove(exposed_name);

        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .remove(&exposed_name.to_string())
            .ok_or_else(|| miette::miette!("The exposed name {exposed_name} doesn't exist"))?;

        tracing::debug!("Removed exposed mapping {exposed_name} from toml document");
        Ok(())
    }

    /// Saves the manifest to the file system
    pub async fn save(&self) -> miette::Result<()> {
        let contents = self.document.to_string();
        tokio_fs::write(&self.path, contents)
            .await
            .into_diagnostic()?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Mapping {
    exposed_name: ExposedName,
    executable_name: String,
}

impl Mapping {
    pub fn new(exposed_name: ExposedName, executable_name: String) -> Self {
        Self {
            exposed_name,
            executable_name,
        }
    }
}

impl fmt::Display for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.exposed_name, self.executable_name)
    }
}

impl FromStr for Mapping {
    type Err = miette::Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        input
            .split_once('=')
            .ok_or_else(|| {
                miette::miette!(
                    "Could not parse mapping `exposed_name=executable_name` from {input}"
                )
            })
            .and_then(|(key, value)| {
                Ok(Mapping::new(ExposedName::from_str(key)?, value.to_string()))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use indexmap::IndexSet;
    use rattler_conda_types::ParseStrictness;

    use super::*;

    #[test]
    fn test_add_exposed_mapping_new_env() {
        let mut manifest = Manifest::default();
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let executable_name = "test_executable".to_string();
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        let result = manifest.add_exposed_mapping(&env_name, &mapping);
        assert!(result.is_ok());

        let expected_value = "test_executable";

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{}.exposed", env_name))
            .unwrap()
            .get(&exposed_name.to_string())
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
            .get(&exposed_name)
            .unwrap();
        assert_eq!(expected_value, actual_value)
    }

    #[test]
    fn test_add_exposed_mapping_existing_env() {
        let mut manifest = Manifest::default();
        let exposed_name1 = ExposedName::from_str("test_exposed1").unwrap();
        let executable_name1 = "test_executable1".to_string();
        let mapping1 = Mapping::new(exposed_name1.clone(), executable_name1);
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        manifest.add_exposed_mapping(&env_name, &mapping1).unwrap();

        let exposed_name2 = ExposedName::from_str("test_exposed2").unwrap();
        let executable_name2 = "test_executable2".to_string();
        let mapping2 = Mapping::new(exposed_name2.clone(), executable_name2);
        let result = manifest.add_exposed_mapping(&env_name, &mapping2);
        assert!(result.is_ok());

        // Check document for executable1
        let expected_value1 = "test_executable1";
        let actual_value1 = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))
            .unwrap()
            .get(&exposed_name1.to_string())
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
            .get(&exposed_name1)
            .unwrap();
        assert_eq!(expected_value1, actual_value1);

        // Check document for executable2
        let expected_value2 = "test_executable2";
        let actual_value2 = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))
            .unwrap()
            .get(&exposed_name2.to_string())
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
            .get(&exposed_name2)
            .unwrap();
        assert_eq!(expected_value2, actual_value2)
    }

    #[test]
    fn test_remove_exposed_mapping() {
        let mut manifest = Manifest::default();
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let executable_name = "test_executable".to_string();
        let mapping = Mapping::new(exposed_name.clone(), executable_name);
        let env_name = EnvironmentName::from_str("test-env").unwrap();

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
            .get(&exposed_name.to_string());
        assert!(actual_value.is_none());

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .exposed
            .get(&exposed_name);
        assert!(actual_value.is_none())
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

        // Ensure no panic and result is Ok
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_dependency() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        let package_name_str = "pythonic";
        let package_name = PackageName::from_str(package_name_str).unwrap();
        let version_spec = "==3.15.0";
        let match_spec = MatchSpec::from_str(
            &format!("{package_name_str}{version_spec}"),
            ParseStrictness::Strict,
        )
        .unwrap();

        // Add dependency
        manifest
            .add_dependency(&env_name, &package_name, &match_spec)
            .unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.dependencies"))
            .unwrap()
            .get(package_name_str);
        assert!(actual_value.is_some());
        assert_eq!(actual_value.unwrap().as_str(), Some(version_spec));

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .dependencies
            .get(&package_name)
            .unwrap()
            .clone();
        assert_eq!(
            actual_value.into_version().unwrap().to_string(),
            version_spec
        );
    }

    #[test]
    fn test_add_existing_dependency() {
        let mut manifest = Manifest::default();
        let env_name = EnvironmentName::from_str("test-env").unwrap();
        let package_name_str = "pythonic";
        let package_name = PackageName::from_str(package_name_str).unwrap();
        let version_spec = "==3.15.0";
        let match_spec = MatchSpec::from_str(
            &format!("{package_name_str}{version_spec}"),
            ParseStrictness::Strict,
        )
        .unwrap();

        // Add dependency
        manifest
            .add_dependency(&env_name, &package_name, &match_spec)
            .unwrap();

        // Add the same dependency again, with a new match_spec
        let new_version_spec = "==3.18.0";
        let new_match_spec = MatchSpec::from_str(
            &format!("{package_name_str}{new_version_spec}"),
            ParseStrictness::Strict,
        )
        .unwrap();
        manifest
            .add_dependency(&env_name, &package_name, &new_match_spec)
            .unwrap();

        // Check document
        let actual_value = manifest
            .document
            .get_or_insert_nested_table(&format!("envs.{env_name}.dependencies"))
            .unwrap()
            .get(package_name_str);
        assert!(actual_value.is_some());
        assert_eq!(actual_value.unwrap().as_str(), Some(new_version_spec));

        // Check parsed
        let actual_value = manifest
            .parsed
            .envs
            .get(&env_name)
            .unwrap()
            .dependencies
            .get(&package_name)
            .unwrap()
            .clone();
        assert_eq!(
            actual_value.into_version().unwrap().to_string(),
            new_version_spec
        );
    }
}
