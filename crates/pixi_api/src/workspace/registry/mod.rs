use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use miette::{Context, IntoDiagnostic};

use std::path::PathBuf;

use pixi_consts::consts;
use pixi_config::pixi_home;


/// Returns the path to the workspace registry file
pub fn workspace_registry_path() -> Option<PathBuf> {
    pixi_home().map(|d| d.join(consts::WORKSPACES_REGISTRY))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct WorkspaceRegistry {
    /// Mapping of a named workspaces to the path of their manifest file.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub named_workspaces: HashMap<String, PathBuf>,
}

impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self {
            named_workspaces: HashMap::new(),
        }
    }
}

impl WorkspaceRegistry {
    /// Loads the workspace registry from disk, if it exists.
    pub async fn load() -> miette::Result<Self> {
        let path = workspace_registry_path()
            .ok_or_else(|| miette::miette!("Unable to determine pixi home directory"))?;

        let contents = match fs_err::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound || e.kind() == std::io::ErrorKind::NotADirectory => {
                // File doesn't exist yet, return default
                return Ok(WorkspaceRegistry::default());
            }
            Err(e) => {
                return Err(e)
                    .into_diagnostic()?;
            }
        };

        let de = toml_edit::de::Deserializer::parse(&contents)
            .into_diagnostic()?;

        // Deserialize the contents
        let registry: WorkspaceRegistry = serde_ignored::deserialize(de, |_| {})
            .into_diagnostic()?;

        Ok(registry)
    }

    /// Saves the workspace registry to disk.
    pub async fn save(&self) -> miette::Result<()> {
        let path = workspace_registry_path()
            .ok_or_else(|| miette::miette!("Unable to determine pixi home directory"))?;


        if let Some(parent) = path.parent() {
            fs_err::create_dir_all(parent)
                .into_diagnostic()
                .wrap_err(format!(
                    "failed to create directories in '{}'",
                    parent.display()
                ))?;
        }

        let contents = toml_edit::ser::to_string_pretty(&self).into_diagnostic()?;
        fs_err::write(&path, contents)
            .into_diagnostic()
            .wrap_err(format!("failed to write config to '{}'", &path.display()))
    }

    /// Remove the workspace from the registry given the workspace name
    pub async fn remove_workspace(&mut self, name: &String) -> miette::Result<()> {
        if self.named_workspaces.contains_key(name) {
            self.named_workspaces.remove(name);
            self.save().await?;
        } else {
            return Err(
                miette::diagnostic!("Workspace '{}' is not found.", name,).into(),
            );
        }
        Ok(())
    }

    /// Add a workspace to the registry given the name and path association
    pub async fn add_workspace(&mut self, name: String, path: PathBuf) -> miette::Result<()> {
        if self.named_workspaces.contains_key(&name) {
            return Err(miette::diagnostic!(
                "Workspace with name '{}' is already registered.",
                name,
            )
            .into());
        } else {
            self.named_workspaces.insert(name, path);
            self.save().await?;
        }
        Ok(())
    }

    /// Get a hashmap of registered workspaces name to path association
    pub fn named_workspaces_map(&self) -> &std::collections::HashMap<String, PathBuf> {
        &self.named_workspaces
    }

    /// Retrieve the path to the manifest file for a named workspaces.
    pub fn named_workspace(&self, name: &String) -> miette::Result<PathBuf> {
        match self.named_workspaces.get(name) {
            Some(path) => Ok(path.clone()),
            None => Err(miette::diagnostic!("Named workspace '{}' not found", name).into()),
        }
    }
}
