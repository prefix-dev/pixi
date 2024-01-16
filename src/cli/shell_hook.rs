use std::path::PathBuf;

use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};

use crate::prefix::Prefix;

/// Prints the activation script to the stdout.
pub async fn execute() -> miette::Result<()> {
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

    println!("{script}");

    Ok(())
}
