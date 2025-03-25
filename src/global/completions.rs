use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::pixi_home;
use rattler_conda_types::PrefixRecord;
use rattler_shell::shell::{Bash, Fish, Shell, Zsh};

use super::Mapping;
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

    pub(crate) async fn contains_completion(&self, completion: Completion) -> bool {
        self.0
            .join(completion.exposed_directory())
            .join(completion.exposed_file_name())
            .is_file()
    }

    /// Returns the path to the binary directory
    pub fn path(&self) -> &Path {
        &self.0
    }
}
pub enum Completion {
    Bash(PathBuf),
    Zsh(PathBuf),
    Fish(PathBuf),
}

impl Completion {
    fn exposed_file_name(&self) -> String {
        let path = match self {
            Self::Bash(path) => path,
            Self::Zsh(path) => path,
            Self::Fish(path) => path,
        };

        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    }

    fn exposed_directory(&self) -> String {
        match self {
            Self::Bash(_) => "bash".to_string(),
            Self::Zsh(_) => "zsh".to_string(),
            Self::Fish(_) => "fish".to_string(),
        }
    }
}

pub fn contained_completions(prefix_root: &Path, name: &str) -> Vec<Completion> {
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
        completion_scripts.push(Completion::Bash(bash_path));
    }

    if zsh_path.exists() {
        completion_scripts.push(Completion::Zsh(zsh_path));
    }

    if fish_path.exists() {
        completion_scripts.push(Completion::Fish(fish_path));
    }
    completion_scripts
}

pub(crate) async fn completions_sync_status(
    exposed_mappings: IndexSet<Mapping>,
    prefix_records: Vec<PrefixRecord>,
    prefix_root: &Path,
    completions_dir: &CompletionsDir,
) -> miette::Result<(Vec<PrefixRecord>, Vec<PrefixRecord>)> {
    let mut records_to_install = Vec::new();
    let mut records_to_uninstall = Vec::new();

    let exposed_names = exposed_mappings
        .into_iter()
        .filter(|mapping| mapping.exposed_name().to_string() == mapping.executable_name())
        .map(|name| name.executable_name().to_string())
        .collect_vec();

    for record in prefix_records {
        let name = record
            .repodata_record
            .package_record
            .name
            .as_normalized()
            .to_string();
        let completions = contained_completions(prefix_root, &name);

        if exposed_names.contains(&name) {
            for completion in completions {
                if !completions_dir.contains_completion(completion).await {
                    records_to_install.push(record.clone());
                }
            }
        } else {
            for completion in completions {
                if completions_dir.contains_completion(completion).await {
                    records_to_uninstall.push(record.clone());
                }
            }
        }
    }

    Ok((records_to_install, records_to_uninstall))
}
