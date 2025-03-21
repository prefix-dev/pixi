use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use rattler_shell::shell::{Bash, Fish, Shell, Zsh};

use super::Mapping;
use crate::prefix::Prefix;

pub enum CompletionScript {
    Zsh(PathBuf),
    Fish(PathBuf),
    Bash(PathBuf),
}

// TODO: use that function in the code
pub fn find_completions(
    prefix: &Prefix,
    executables: &IndexSet<Mapping>,
) -> miette::Result<Vec<CompletionScript>> {
    let mut completion_scripts = Vec::new();

    for exe in executables {
        if exe.executable_name() != exe.exposed_name().as_ref() {
            // If the executable name is not the same as the exposed name, we don't do anything
            // The completion script would have to be adjusted to match whatever the exposed name is
            continue;
        }

        completion_scripts.extend(contained_completions(prefix.root(), exe));
    }

    Ok(completion_scripts)
}

fn contained_completions(root: &Path, exe: &Mapping) -> Vec<CompletionScript> {
    let mut completion_scripts = Vec::new();

    let name = exe.executable_name();
    let zsh_name = format!("_{name}");
    let fish_name = format!("{name}.fish");

    let bash_path = root
        .join(Bash.completion_script_location().expect("is set"))
        .join(name);
    let zsh_path = root
        .join(Zsh.completion_script_location().expect("is set"))
        .join(zsh_name);
    let fish_path = root
        .join(Fish.completion_script_location().expect("is set"))
        .join(fish_name);

    if bash_path.exists() {
        completion_scripts.push(CompletionScript::Bash(bash_path));
    }

    if zsh_path.exists() {
        completion_scripts.push(CompletionScript::Zsh(zsh_path));
    }

    if fish_path.exists() {
        completion_scripts.push(CompletionScript::Fish(fish_path));
    }
    completion_scripts
}
