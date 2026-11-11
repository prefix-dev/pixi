/// This module contains code facilitating shell completion support for `pixi global`
use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use itertools::Itertools;
use pixi_config::pixi_home;
use rattler_shell::shell::{Bash, Fish, Shell as _, Zsh};

use super::Mapping;
use super::StateChange;
use fs_err::tokio as tokio_fs;

/// An error from managing the global shell completions.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum CompletionsError {
    #[error("couldn't determine the global completions directory")]
    MissingPixiHome,

    #[error("failed to create the global completions directory")]
    CreateDir(#[source] std::io::Error),

    #[error("failed to prune old completions")]
    Prune(#[source] std::io::Error),

    #[error("failed to install the completion")]
    Install(#[source] std::io::Error),

    #[error("failed to remove the completion")]
    Remove(#[source] std::io::Error),
}

/// Global completions directory, default to `$HOME/.pixi/completions`
#[derive(Debug, Clone)]
pub struct CompletionsDir(PathBuf);

impl CompletionsDir {
    /// Create the global complations directory from environment variables
    pub async fn from_env() -> Result<Self, CompletionsError> {
        let bin_dir = pixi_home()
            .map(|path| path.join("completions"))
            .ok_or(CompletionsError::MissingPixiHome)?;
        tokio_fs::create_dir_all(&bin_dir)
            .await
            .map_err(CompletionsError::CreateDir)?;
        Ok(Self(bin_dir))
    }

    /// Returns the path to the binary directory
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Prune old completions
    pub fn prune_old_completions(&self) -> Result<(), CompletionsError> {
        for directory in [self.bash_path(), self.zsh_path(), self.fish_path()] {
            if !directory.is_dir() {
                continue;
            }

            for entry in fs_err::read_dir(&directory).map_err(CompletionsError::Prune)? {
                let path = entry.map_err(CompletionsError::Prune)?.path();

                if (path.is_symlink() && fs_err::read_link(&path).is_err())
                    || (!path.is_symlink() && path.is_file())
                {
                    // Remove broken symlink
                    fs_err::remove_file(&path).map_err(CompletionsError::Prune)?;
                }
            }
        }

        Ok(())
    }

    pub fn bash_path(&self) -> PathBuf {
        self.path().join("bash")
    }

    pub fn zsh_path(&self) -> PathBuf {
        self.path().join("zsh")
    }

    pub fn fish_path(&self) -> PathBuf {
        self.path().join("fish")
    }
}

#[derive(Debug, Clone)]
pub struct Completion {
    name: String,
    source: PathBuf,
    destination: PathBuf,
}

impl Completion {
    pub fn new(name: String, source: PathBuf, destination: PathBuf) -> Self {
        Self {
            name,
            source,
            destination,
        }
    }

    /// Install the shell completion
    pub async fn install(&self) -> Result<Option<StateChange>, CompletionsError> {
        tracing::debug!("Requested to install completion {}.", self.source.display());

        // Ensure the parent directory of the destination exists
        if let Some(parent) = self.destination.parent() {
            tokio_fs::create_dir_all(parent)
                .await
                .map_err(CompletionsError::Install)?;
        }

        // Attempt to create the symlink
        tokio_fs::symlink(&self.source, &self.destination)
            .await
            .map_err(CompletionsError::Install)?;

        Ok(Some(StateChange::AddedCompletion(self.name.clone())))
    }

    /// Remove the shell completion
    pub async fn remove(&self) -> Result<StateChange, CompletionsError> {
        tokio_fs::remove_file(&self.destination)
            .await
            .map_err(CompletionsError::Remove)?;

        Ok(StateChange::RemovedCompletion(self.name.clone()))
    }
}

/// Generates a list of shell completion scripts for a given executable name.
///
/// This function checks for the existence of shell completion scripts for Bash, Zsh, and Fish
/// in the specified `prefix_root` directory. If the scripts exist, it creates a list of
/// `Completion` objects that represent the source and destination paths for these scripts.
pub fn contained_completions(
    prefix_root: &Path,
    name: &str,
    completions_dir: &CompletionsDir,
) -> Vec<Completion> {
    let mut completion_scripts = Vec::new();

    let zsh_name = format!("_{name}");
    let fish_name = format!("{name}.fish");

    let bash_path = prefix_root
        .join(
            Bash::default()
                .completion_script_location()
                .expect("bash has a completion script location"),
        )
        .join(name);
    let zsh_path = prefix_root
        .join(
            Zsh.completion_script_location()
                .expect("zsh has a completion script location"),
        )
        .join(&zsh_name);
    let fish_path = prefix_root
        .join(
            Fish.completion_script_location()
                .expect("fish has a completion script location"),
        )
        .join(&fish_name);

    if bash_path.exists() {
        let destination = completions_dir.bash_path().join(name);
        completion_scripts.push(Completion::new(name.to_string(), bash_path, destination));
    }

    if zsh_path.exists() {
        let destination = completions_dir.zsh_path().join(zsh_name);
        completion_scripts.push(Completion::new(name.to_string(), zsh_path, destination));
    }

    if fish_path.exists() {
        let destination = completions_dir.fish_path().join(fish_name);
        completion_scripts.push(Completion::new(name.to_string(), fish_path, destination));
    }
    completion_scripts
}

/// Synchronizes the shell completion scripts for the given executable names.
///
/// This function determines which shell completion scripts need to be removed or added
/// based on the provided `exposed_mappings` and `executable_names`. It compares the
/// current state of the completion scripts in the `completions_dir` with the expected
/// state derived from the `exposed_mappings`.
pub(crate) async fn completions_sync_status(
    exposed_mappings: IndexSet<Mapping>,
    executable_names: Vec<String>,
    prefix_root: &Path,
    completions_dir: &CompletionsDir,
) -> (Vec<Completion>, Vec<Completion>) {
    let mut completions_to_add = Vec::new();
    let mut completions_to_remove = Vec::new();

    let exposed_names = exposed_mappings
        .into_iter()
        .filter(|mapping| mapping.exposed_name().to_string() == mapping.executable_name())
        .map(|name| name.executable_name().to_string())
        .collect_vec();

    for name in executable_names.into_iter().unique() {
        let completions = contained_completions(prefix_root, &name, completions_dir);

        if completions.is_empty() {
            continue;
        }

        if exposed_names.contains(&name) {
            for completion in completions {
                if !completion.destination.is_symlink() {
                    completions_to_add.push(completion);
                }
            }
        } else {
            for completion in completions {
                if completion.destination.is_symlink()
                    && let Ok(target) = tokio_fs::read_link(&completion.destination).await
                    && target == completion.source
                {
                    completions_to_remove.push(completion);
                }
            }
        }
    }

    (completions_to_remove, completions_to_add)
}
