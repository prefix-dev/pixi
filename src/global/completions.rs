use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::pixi_home;
use rattler_conda_types::PrefixRecord;
use rattler_shell::shell::{Bash, Fish, Shell as _, Zsh};

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

    /// Returns the path to the binary directory
    pub fn path(&self) -> &Path {
        &self.0
    }
}

pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    fn exposed_directory(&self) -> String {
        match self {
            Self::Bash => "bash".to_string(),
            Self::Zsh => "zsh".to_string(),
            Self::Fish => "fish".to_string(),
        }
    }
}

pub struct Completion {
    name: String,
    source: PathBuf,
    destination: PathBuf,
    shell: Shell,
}

impl Completion {
    pub fn new(
        name: String,
        source: PathBuf,
        completions_dir: &CompletionsDir,
        shell: Shell,
    ) -> Self {
        let destination = completions_dir
            .path()
            .join(shell.exposed_directory())
            .join(Self::exposed_file_name(&source));

        Self {
            name,
            source,
            destination,
            shell,
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
        completion_scripts.push(Completion::new(
            name.to_string(),
            bash_path,
            completions_dir,
            Shell::Bash,
        ));
    }

    if zsh_path.exists() {
        completion_scripts.push(Completion::new(
            name.to_string(),
            zsh_path,
            completions_dir,
            Shell::Zsh,
        ));
    }

    if fish_path.exists() {
        completion_scripts.push(Completion::new(
            name.to_string(),
            fish_path,
            completions_dir,
            Shell::Fish,
        ));
    }
    completion_scripts
}

pub(crate) async fn completions_sync_status(
    exposed_mappings: IndexSet<Mapping>,
    prefix_records: Vec<PrefixRecord>,
    prefix_root: &Path,
    completions_dir: &CompletionsDir,
) -> miette::Result<(Vec<Completion>, Vec<Completion>)> {
    let mut completions_to_install = Vec::new();
    let mut completions_to_uninstall = Vec::new();

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
        let completions = contained_completions(prefix_root, &name, completions_dir);

        if exposed_names.contains(&name) {
            for completion in completions {
                if !completion.destination.is_file() {
                    completions_to_install.push(completion);
                }
            }
        } else {
            for completion in completions {
                if completion.destination.is_file() {
                    completions_to_uninstall.push(completion);
                }
            }
        }
    }

    Ok((completions_to_install, completions_to_uninstall))
}
