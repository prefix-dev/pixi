use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};

use crate::{
    environment::{get_up_to_date_prefix_from_environment, LockFileUsage},
    prefix::Prefix,
    Project,
};

/// Print the activation script
#[derive(Parser, Debug)]
pub struct Args {
    /// Sets the shell
    #[arg(short, long)]
    shell: Option<ShellEnum>,
}

/// Generates the activation script.
async fn generate_activation_script(shell: Option<ShellEnum>) -> miette::Result<String> {
    let project = Project::discover()?;
    let environment = project.default_environment();

    get_up_to_date_prefix_from_environment(
        &environment,
        LockFileUsage::Frozen,
        false,
        None,
        Default::default(),
    )
    .await?;

    let platform = Platform::current();
    let prefix = Prefix::new(environment.dir())?;
    let shell = shell.unwrap_or_default();
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
            path_modification_behavior: PathModificationBehavior::default(),
        })
        .into_diagnostic()?;

    Ok(result.script.clone())
}

/// Prints the activation script to the stdout.
pub async fn execute(args: Args) -> miette::Result<()> {
    let script = generate_activation_script(args.shell).await?;
    println!("{script}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shell_hook() {
        let script = generate_activation_script(None).await.unwrap();
        if cfg!(unix) {
            assert!(script.contains("export PATH="));
            assert!(script.contains("export CONDA_PREFIX="));
        } else {
            assert!(script.contains("@SET \"PATH="));
            assert!(script.contains("@SET \"CONDA_PREFIX="));
        }
    }
}
