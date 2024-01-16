use std::path::PathBuf;

use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};

use crate::prefix::Prefix;

/// Generates the activation script.
fn generate_activation_script() -> miette::Result<String> {
    let prefix = Prefix::new(PathBuf::new())?;
    let shell: ShellEnum = if cfg!(windows) {
        rattler_shell::shell::CmdExe.into()
    } else {
        rattler_shell::shell::Bash.into()
    };
    let platform = Platform::current();

    let activator = Activator::from_path(prefix.root(), shell, platform).into_diagnostic()?;
    let path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());

    // If we are in a conda environment, we need to deactivate it before activating the host / build prefix
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(|p| p.into());
    let result = activator
        .activation(ActivationVariables {
            conda_prefix,
            path,
            path_modification_behavior: Default::default(),
        })
        .into_diagnostic()?;

    // Add a shebang on unix based platforms
    let script = if cfg!(unix) {
        format!("#!/bin/sh\n{}", result.script)
    } else {
        result.script
    };

    Ok(script)
}

/// Prints the activation script to the stdout.
pub fn execute() -> miette::Result<()> {
    let script = generate_activation_script()?;
    println!("{script}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn test_shell_hook() {
        let script = generate_activation_script().unwrap();
        if cfg!(unix) {
            assert!(script.starts_with("#!/bin/sh"));
        }
        assert!(script.contains("export PATH="));
        assert!(script.contains("export CONDA_PREFIX="));
    }
}
