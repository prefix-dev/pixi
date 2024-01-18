use std::path::PathBuf;

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};

use crate::{prefix::Prefix, Project};

/// Print the activation script
#[derive(Parser, Debug)]
pub struct Args {
    /// Sets the shell
    #[arg(short, long)]
    shell: Option<ShellEnum>,
}

/// Generates the activation script.
fn generate_activation_script(shell: Option<ShellEnum>) -> miette::Result<String> {
    let project = Project::discover()?;
    let platform = Platform::current();
    let env_dir = project.default_environment().dir();
    let prefix = Prefix::new(PathBuf::new())?;
    let shell = shell.unwrap_or_default();
    let activator = Activator::from_path(prefix.root(), shell, platform).into_diagnostic()?;
    let mut path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    if let Some(path) = path.as_mut() {
        path.push(env_dir);
    } else {
        path = Some(vec![env_dir]);
    }

    // If we are in a conda environment, we need to deactivate it before activating the host / build prefix
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(|p| p.into());
    let result = activator
        .activation(ActivationVariables {
            conda_prefix,
            path,
            path_modification_behavior: Default::default(),
        })
        .into_diagnostic()?;

    Ok(result.script.clone())
}

/// Prints the activation script to the stdout.
pub fn execute(args: Args) -> miette::Result<()> {
    let script = generate_activation_script(args.shell)?;
    println!("{script}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn test_shell_hook() {
        let script = generate_activation_script(None).unwrap();
        if cfg!(unix) {
            assert!(script.contains("export PATH="));
            assert!(script.contains("export CONDA_PREFIX="));
        } else {
            assert!(script.contains("@SET \"PATH="));
            assert!(script.contains("@SET \"CONDA_PREFIX="));
        }
    }
}
