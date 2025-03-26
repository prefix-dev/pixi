use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::pixi_home;
use rattler_shell::shell::{Bash, Fish, Shell as _, Zsh};

use super::Mapping;
use super::StateChange;
use fs_err::tokio as tokio_fs;

/// Global completions directory, default to `$HOME/.pixi/completions`
#[derive(Debug, Clone)]
pub struct CompletionsDir(PathBuf);

impl CompletionsDir {
    /// Create the global complations directory from environment variables
    pub async fn from_env() -> miette::Result<Self> {
        let bin_dir = pixi_home()
            .map(|path| path.join("completions"))
            .ok_or(miette::miette!(
                "Couldn't determine global completions directory"
            ))?;
        tokio_fs::create_dir_all(&bin_dir).await.into_diagnostic()?;
        Ok(Self(bin_dir))
    }

    /// Returns the path to the binary directory
    pub fn path(&self) -> &Path {
        &self.0
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

    pub fn name(&self) -> &str {
        &self.name
    }

    fn exposed_file_name(source: &Path) -> String {
        source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    }

    pub async fn install(&self) -> miette::Result<Option<StateChange>> {
        if cfg!(unix) {
            // Ensure the parent directory of the destination exists
            if let Some(parent) = self.destination.parent() {
                tokio_fs::create_dir_all(parent).await.into_diagnostic()?;
            }

            // Attempt to create the symlink
            tokio_fs::symlink(&self.source, &self.destination)
                .await
                .into_diagnostic()?;

            Ok(Some(StateChange::AddedCompletion(self.name.clone())))
        } else {
            tracing::info!(
                "Symlinks are only supported on unix-like platforms. Skipping completion installation for {}.",
                self.name
            );
            Ok(None)
        }
    }

    pub async fn remove(&self) -> miette::Result<StateChange> {
        tokio_fs::remove_file(&self.destination)
            .await
            .into_diagnostic()?;

        Ok(StateChange::RemovedCompletion(self.name.clone()))
    }
}

pub fn contained_completions(
    prefix_root: &Path,
    name: &str,
    completions_dir: &CompletionsDir,
) -> Vec<Completion> {
    let mut completion_scripts = Vec::new();

    let zsh_name = format!("_{name}");
    let fish_name = format!("{name}.fish");

    let bash_path = prefix_root
        .join(Bash.completion_script_location().expect("is set"))
        .join(name);
    let zsh_path = prefix_root
        .join(Zsh.completion_script_location().expect("is set"))
        .join(zsh_name);
    let fish_path = prefix_root
        .join(Fish.completion_script_location().expect("is set"))
        .join(fish_name);

    if bash_path.exists() {
        let destination = completions_dir
            .path()
            .join("bash")
            .join(Completion::exposed_file_name(&bash_path));

        completion_scripts.push(Completion::new(name.to_string(), bash_path, destination));
    }

    if zsh_path.exists() {
        let destination = completions_dir
            .path()
            .join("zsh")
            .join(Completion::exposed_file_name(&zsh_path));
        completion_scripts.push(Completion::new(name.to_string(), zsh_path, destination));
    }

    if fish_path.exists() {
        let destination = completions_dir
            .path()
            .join("fish")
            .join(Completion::exposed_file_name(&fish_path));
        completion_scripts.push(Completion::new(name.to_string(), fish_path, destination));
    }
    completion_scripts
}

pub(crate) async fn completions_sync_status(
    exposed_mappings: IndexSet<Mapping>,
    executable_names: Vec<String>,
    prefix_root: &Path,
    completions_dir: &CompletionsDir,
) -> miette::Result<(Vec<Completion>, Vec<Completion>)> {
    let mut completions_to_add = Vec::new();
    let mut completions_to_remove = Vec::new();

    let exposed_names = exposed_mappings
        .into_iter()
        .filter(|mapping| mapping.exposed_name().to_string() == mapping.executable_name())
        .map(|name| name.executable_name().to_string())
        .collect_vec();

    for name in executable_names {
        let completions = contained_completions(prefix_root, &name, completions_dir);

        if completions.is_empty() {
            continue;
        }

        if exposed_names.contains(&name) {
            for completion in completions {
                if !completion.destination.is_file() {
                    completions_to_add.push(completion);
                }
            }
        } else {
            for completion in completions {
                if completion.destination.is_file() {
                    completions_to_remove.push(completion);
                }
            }
        }
    }

    Ok((completions_to_remove, completions_to_add))
}
