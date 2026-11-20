use std::path::PathBuf;
use std::{cmp::Ordering, collections::HashMap};

use itertools::Itertools;
use miette::Diagnostic;
use pixi_config::pixi_home;
use pixi_consts::consts;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Returns the path to the workspace registry file
pub fn workspace_registry_path() -> Option<PathBuf> {
    pixi_home().map(|d| d.join(consts::WORKSPACES_REGISTRY))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub struct WorkspaceRegistry {
    /// Mapping of a named workspaces to the path of their manifest file.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub named_workspaces: HashMap<String, PathBuf>,
}

/// Errors that may occur when using the workspace registry
#[derive(Debug, Error, Diagnostic)]
pub enum WorkspaceRegistryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("unable to determine pixi home directory")]
    PixiHome,

    #[error("failed to parse the workspace registry")]
    Parse(#[source] toml_edit::de::Error),

    #[error("failed to serialize the workspace registry")]
    Serialize(#[source] toml_edit::ser::Error),

    #[error("failed to create directories in '{}'", .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write workspace registry config to '{}'", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not find workspace '{}'", .name)]
    #[diagnostic(help("{help}"))]
    MissingWorkspace { name: String, help: String },
}

impl WorkspaceRegistry {
    /// Loads the workspace registry from disk, if it exists.
    pub fn load() -> Result<Self, WorkspaceRegistryError> {
        let path = workspace_registry_path().ok_or(WorkspaceRegistryError::PixiHome)?;

        let contents = match fs_err::read_to_string(&path) {
            Ok(c) => c,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::NotADirectory =>
            {
                // File doesn't exist yet, return default
                return Ok(WorkspaceRegistry::default());
            }
            Err(e) => {
                return Err(e.into());
            }
        };

        // Deserialize the contents
        let de =
            toml_edit::de::Deserializer::parse(&contents).map_err(WorkspaceRegistryError::Parse)?;
        let registry: WorkspaceRegistry =
            serde_ignored::deserialize(de, |_| {}).map_err(WorkspaceRegistryError::Parse)?;

        Ok(registry)
    }

    /// Saves the workspace registry to disk.
    pub async fn save(&self) -> Result<(), WorkspaceRegistryError> {
        let path = workspace_registry_path().ok_or(WorkspaceRegistryError::PixiHome)?;

        if let Some(parent) = path.parent() {
            fs_err::create_dir_all(parent).map_err(|source| WorkspaceRegistryError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let contents =
            toml_edit::ser::to_string_pretty(&self).map_err(WorkspaceRegistryError::Serialize)?;
        fs_err::write(&path, contents)
            .map_err(|source| WorkspaceRegistryError::Write { path, source })
    }

    /// Remove the workspace from the registry given the workspace name.
    pub async fn remove_workspace(&mut self, name: &String) -> Result<(), WorkspaceRegistryError> {
        self.named_workspace(name)?;
        self.named_workspaces.remove(name);
        self.save().await
    }

    /// Add a workspace to the registry given the name and path association.
    pub async fn add_workspace(
        &mut self,
        name: String,
        path: PathBuf,
    ) -> Result<(), WorkspaceRegistryError> {
        self.named_workspaces.entry(name).or_insert(path);
        self.save().await
    }

    /// Prune the workspace by removing entries whose path does not exist. Returns the
    /// list of workspaces that have been removed.
    pub async fn prune(&mut self) -> Result<Vec<String>, WorkspaceRegistryError> {
        let names_to_remove: Vec<_> = self
            .named_workspaces
            .iter()
            .filter(|(_, path)| !path.exists())
            .map(|(name, _)| name.clone())
            .collect();

        for name in &names_to_remove {
            self.named_workspaces.remove(name);
        }

        self.save().await?;
        Ok(names_to_remove)
    }

    /// Get a hashmap of registered workspaces name to path association.
    pub fn named_workspaces_map(&self) -> &std::collections::HashMap<String, PathBuf> {
        &self.named_workspaces
    }

    /// Retrieve the path to the manifest file for a named workspaces.
    pub fn named_workspace(&self, name: &String) -> Result<PathBuf, WorkspaceRegistryError> {
        match self.named_workspaces.get(name) {
            Some(path) => Ok(path.clone()),
            None => {
                let similar_names = self
                    .named_workspaces
                    .iter()
                    .map(|p| p.0.to_string())
                    .filter_map(|workspace_name: String| {
                        let distance = strsim::jaro(&workspace_name, name);
                        if distance > 0.6 {
                            Some((workspace_name, distance))
                        } else {
                            None
                        }
                    })
                    .sorted_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(Ordering::Equal))
                    .take(5)
                    .map(|(name, _)| name)
                    .collect_vec();

                let help = if !similar_names.is_empty() {
                    format!("did you mean '{}'?", similar_names.iter().format("', '"))
                } else {
                    "use `pixi workspace register list` to view all available workspaces."
                        .to_string()
                };
                Err(WorkspaceRegistryError::MissingWorkspace {
                    name: name.to_string(),
                    help,
                })
            }
        }
    }

    /// Check if the name of the workspace is already registered.
    pub fn contains_workspace(&self, name: &String) -> bool {
        self.named_workspaces.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_registry() {
        let registry = WorkspaceRegistry::default();
        assert!(registry.named_workspaces.is_empty());
        assert_eq!(registry.named_workspaces_map().len(), 0);
    }

    #[test]
    fn test_contains_workspace() {
        let mut registry = WorkspaceRegistry::default();

        registry
            .named_workspaces
            .insert("test-ws".to_string(), PathBuf::from("/tmp/workspace"));

        assert!(registry.contains_workspace(&"test-ws".to_string()));
        assert!(!registry.contains_workspace(&"other-ws".to_string()));
    }

    #[test]
    fn test_named_workspace() {
        let mut registry = WorkspaceRegistry::default();
        let path = PathBuf::from("/tmp/workspace");

        registry
            .named_workspaces
            .insert("test-ws".to_string(), path.clone());

        let result = registry.named_workspace(&"test-ws".to_string());
        assert_eq!(result.unwrap(), path);
    }

    #[test]
    fn test_named_workspace_not_found() {
        let registry = WorkspaceRegistry::default();
        let result = registry.named_workspace(&"non-existent".to_string());
        assert!(result.is_err());
    }
}
