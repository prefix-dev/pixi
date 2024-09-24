use std::fmt;
use std::path::{Path, PathBuf};

use miette::IntoDiagnostic;
use pixi_manifest::{TomlError, TomlManifest};
use toml_edit::{DocumentMut, Item};

use super::parsed_manifest::ParsedManifest;
use super::{EnvironmentName, ExposedName, MANIFEST_DEFAULT_NAME};

/// Handles the global project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`ParsedManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// Editable toml document
    pub document: TomlManifest,

    /// The parsed manifest
    pub parsed: ParsedManifest,
}

impl Manifest {
    /// Create a new manifest from a path
    pub fn from_path(path: impl AsRef<Path>) -> miette::Result<Self> {
        let manifest_path = dunce::canonicalize(path.as_ref()).into_diagnostic()?;
        let contents = std::fs::read_to_string(path.as_ref()).into_diagnostic()?;
        Self::from_str(manifest_path.as_ref(), contents)
    }

    /// Create a new manifest from a string
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

    pub fn add_exposed_mapping(
        &mut self,
        env_name: &EnvironmentName,
        mapping: &Mapping,
    ) -> miette::Result<()> {
        self.parsed
            .envs
            .entry(env_name.clone())
            .or_default()
            .exposed
            .insert(
                mapping.exposed_name.clone(),
                mapping.executable_name.clone(),
            );

        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .insert(
                &mapping.exposed_name.to_string(),
                Item::Value(toml_edit::Value::from(mapping.executable_name.clone())),
            );

        tracing::debug!("Added exposed mapping {mapping} to toml document");
        Ok(())
    }

    pub fn remove_exposed_name(
        &mut self,
        env_name: &EnvironmentName,
        exposed_name: &ExposedName,
    ) -> miette::Result<()> {
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

    /// Save the manifest to the file and update the parsed_manifest
    pub async fn save(&mut self) -> miette::Result<()> {
        let contents = self.document.to_string();
        self.parsed = ParsedManifest::from_toml_str(&contents)?;
        tokio::fs::write(&self.path, contents)
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_add_exposed_mapping_new_env() {
        let mut manifest = Manifest::from_str(&PathBuf::from("pixi-global.toml"), "").unwrap();
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
        let mut manifest = Manifest::from_str(&PathBuf::from("pixi-global.toml"), "").unwrap();
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
        let mut manifest = Manifest::from_str(&PathBuf::from("pixi-global.toml"), "").unwrap();
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
        let mut manifest = Manifest::from_str(&PathBuf::from("pixi-global.toml"), "").unwrap();
        let exposed_name = ExposedName::from_str("test_exposed").unwrap();
        let env_name = EnvironmentName::from_str("test-env").unwrap();

        // Removing an exposed name that doesn't exist should return an error
        let result = manifest.remove_exposed_name(&env_name, &exposed_name);
        assert!(result.is_err())
    }
}
