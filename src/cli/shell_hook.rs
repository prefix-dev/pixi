use clap::Parser;
use indexmap::IndexMap;
use miette::IntoDiagnostic;
use rattler_shell::{
    activation::{ActivationVariables, PathModificationBehavior},
    shell::ShellEnum,
};
use std::{default::Default, path::PathBuf};

use crate::{
    activation::get_activator,
    cli::LockFileUsageArgs,
    environment::get_up_to_date_prefix,
    project::{manifest::EnvironmentName, Environment},
    Project,
};

/// Print the activation script so users can source it in their shell, without needing the pixi executable.
#[derive(Parser, Debug)]
pub struct Args {
    /// Sets the shell, options: [`bash`,  `zsh`,  `xonsh`,  `cmd`,  `powershell`,  `fish`,  `nushell`]
    #[arg(short, long)]
    shell: Option<ShellEnum>,

    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    lock_file_usage: LockFileUsageArgs,

    /// The environment to activate in the script
    #[arg(long, short)]
    environment: Option<String>,
}

/// Generates the activation script.
async fn generate_activation_script(
    shell: Option<ShellEnum>,
    environment: &Environment<'_>,
) -> miette::Result<String> {
    // Get shell from the arguments or from the current process or use default if all fails
    let shell = shell.unwrap_or_else(|| {
        ShellEnum::from_parent_process()
            .unwrap_or_else(|| ShellEnum::from_env().unwrap_or_default())
    });

    let activator = get_activator(environment, shell).into_diagnostic()?;

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

    Ok(result
        .script
        .contents()
        .expect("Could not format the script"))
}

/// Prints the activation script to the stdout.
pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment_name = args
        .environment
        .map_or_else(|| EnvironmentName::Default, EnvironmentName::Named);
    let environment = project
        .environment(&environment_name)
        .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))?;

    get_up_to_date_prefix(
        &environment,
        args.lock_file_usage.into(),
        false,
        IndexMap::default(),
    )
    .await?;

    let script = generate_activation_script(args.shell, &environment).await?;

    // Print the script
    println!("{}", script);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_shell::shell::{Bash, CmdExe, Fish, NuShell, PowerShell, Xonsh, Zsh};

    #[tokio::test]
    async fn test_shell_hook() {
        let project = Project::discover().unwrap();
        let environment = project.default_environment();
        let script = generate_activation_script(Some(ShellEnum::Bash(Bash)), &environment)
            .await
            .unwrap();
        assert!(script.contains("export PATH="));
        assert!(script.contains("export CONDA_PREFIX="));

        let script = generate_activation_script(
            Some(ShellEnum::PowerShell(PowerShell::default())),
            &environment,
        )
        .await
        .unwrap();
        assert!(script.contains("${Env:PATH}"));
        assert!(script.contains("${Env:CONDA_PREFIX}"));

        let script = generate_activation_script(Some(ShellEnum::Zsh(Zsh)), &environment)
            .await
            .unwrap();
        assert!(script.contains("export PATH="));
        assert!(script.contains("export CONDA_PREFIX="));

        let script = generate_activation_script(Some(ShellEnum::Fish(Fish)), &environment)
            .await
            .unwrap();
        assert!(script.contains("set -gx PATH "));
        assert!(script.contains("set -gx CONDA_PREFIX "));

        let script = generate_activation_script(Some(ShellEnum::Xonsh(Xonsh)), &environment)
            .await
            .unwrap();
        assert!(script.contains("$PATH = "));
        assert!(script.contains("$CONDA_PREFIX = "));

        let script = generate_activation_script(Some(ShellEnum::CmdExe(CmdExe)), &environment)
            .await
            .unwrap();
        assert!(script.contains("@SET \"PATH="));
        assert!(script.contains("@SET \"CONDA_PREFIX="));

        let script = generate_activation_script(Some(ShellEnum::NuShell(NuShell)), &environment)
            .await
            .unwrap();
        assert!(script.contains("$env.PATH = "));
        assert!(script.contains("$env.CONDA_PREFIX = "));
    }
}
