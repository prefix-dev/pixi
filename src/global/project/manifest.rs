use std::path::{Path, PathBuf};

use miette::IntoDiagnostic;
use pixi_manifest::TomlManifest;
use rattler_conda_types::{MatchSpec, PackageName};
use toml_edit::{DocumentMut, Item};

// use crate::global::document::ManifestSource;

use super::error::ManifestError;

use super::parsed_manifest::ParsedManifest;
use super::{EnvironmentName, ExposedKey, MANIFEST_DEFAULT_NAME};
// use super::document::ManifestSource;

// TODO: remove
#[allow(unused)]

/// Handles the global project's manifest file.
/// This struct is responsible for reading, parsing, editing, and saving the
/// manifest. It encapsulates all logic related to the manifest's TOML format
/// and structure. The manifest data is represented as a [`ParsedManifest`]
/// struct for easy manipulation.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The path to the manifest file
    pub path: PathBuf,

    /// The raw contents of the manifest file
    pub contents: String,

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
                .map_err(ManifestError::from)
        }) {
            Ok(result) => result,
            Err(e) => e.to_fancy(MANIFEST_DEFAULT_NAME, &contents)?,
        };

        let manifest = Self {
            path: manifest_path.to_path_buf(),
            contents,
            document: TomlManifest::new(document),
            parsed: manifest,
        };

        Ok(manifest)
    }

    /// Adds an environment to the project.
    pub fn add_environment(&mut self, _name: String) -> miette::Result<()> {
        todo!()
    }

    /// Removes an environment from the project.
    pub fn remove_environment(&mut self, _name: &str) -> miette::Result<bool> {
        todo!()
    }

    /// Add a matchspec to the manifest
    pub fn add_dependency(&mut self, _spec: &MatchSpec) -> miette::Result<bool> {
        todo!()
    }

    /// Removes a dependency based on `SpecType`.
    pub fn remove_dependency(&mut self, _dep: &PackageName) -> miette::Result<()> {
        todo!()
    }

    pub fn add_exposed_binary(
        &mut self,
        env_name: &EnvironmentName,
        exposed_name: ExposedKey,
        executable_name: String,
    ) -> miette::Result<()> {
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .insert(
                exposed_name.as_str(),
                Item::Value(toml_edit::Value::from(executable_name.clone())),
            );

        tracing::debug!("Added exposed mapping {exposed_name}={executable_name} to toml document");
        Ok(())
    }

    pub fn remove_exposed_binary(
        &mut self,
        env_name: &EnvironmentName,
        exposed_name: &ExposedKey,
    ) -> miette::Result<()> {
        self.document
            .get_or_insert_nested_table(&format!("envs.{env_name}.exposed"))?
            .remove(exposed_name.as_str());

        tracing::debug!("Removed exposed mapping {exposed_name} from toml document");
        Ok(())
    }

    /// Save the manifest to the file and update the contents
    pub fn save(&mut self) -> miette::Result<()> {
        self.contents = self.document.to_string();
        std::fs::write(&self.path, self.contents.clone()).into_diagnostic()?;
        Ok(())
    }
}
