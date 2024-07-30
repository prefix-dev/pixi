use std::{collections::HashMap, default::Default};

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::ConfigCliPrompt;
use rattler_shell::{
    activation::{ActivationVariables, PathModificationBehavior},
    shell::ShellEnum,
};
use serde::Serialize;
use serde_json;

use crate::cli::cli_config::ProjectConfig;
use crate::{
    activation::{get_activator, CurrentEnvVarBehavior},
    cli::LockFileUsageArgs,
    environment::get_up_to_date_prefix,
    project::Environment,
    HasFeatures, Project,
};

/// Print the pixi environment activation script.
///
/// You can source the script to activate the environment without needing pixi
/// itself.
#[derive(Parser, Debug)]
pub struct Args {
    /// Sets the shell, options: [`bash`,  `zsh`,  `xonsh`,  `cmd`,
    /// `powershell`,  `fish`,  `nushell`]
    #[arg(short, long)]
    shell: Option<ShellEnum>,

    #[clap(flatten)]
    pub project_config: ProjectConfig,

    #[clap(flatten)]
    lock_file_usage: LockFileUsageArgs,

    /// The environment to activate in the script
    #[arg(long, short)]
    environment: Option<String>,

    /// Emit the environment variables set by running the activation as JSON
    #[clap(long, default_value = "false", conflicts_with = "shell")]
    json: bool,

    #[clap(flatten)]
    config: ConfigCliPrompt,
}

#[derive(Serialize)]
struct ShellEnv<'a> {
    environment_variables: &'a HashMap<String, String>,
}

/// Generates the activation script.
async fn generate_activation_script(
    shell: Option<ShellEnum>,
    environment: &Environment<'_>,
) -> miette::Result<String> {
    // Get shell from the arguments or from the current process or use default if
    // all fails
    let shell = shell.unwrap_or_else(|| {
        ShellEnum::from_parent_process()
            .unwrap_or_else(|| ShellEnum::from_env().unwrap_or_default())
    });

    let activator = get_activator(environment, shell).into_diagnostic()?;

    let path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());

    // If we are in a conda environment, we need to deactivate it before activating
    // the host / build prefix
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(|p| p.into());
    let result = activator
        .activation(ActivationVariables {
            conda_prefix,
            path,
            path_modification_behavior: PathModificationBehavior::default(),
        })
        .into_diagnostic()?;

    result.script.contents().into_diagnostic()
}

/// Generates a JSON object describing the changes to the shell environment when
/// activating the provided pixi environment.
async fn generate_environment_json(environment: &Environment<'_>) -> miette::Result<String> {
    let environment_variables = environment
        .project()
        .get_activated_environment_variables(environment, CurrentEnvVarBehavior::Exclude)
        .await?;

    let shell_env = ShellEnv {
        environment_variables,
    };

    serde_json::to_string(&shell_env).into_diagnostic()
}

/// Prints the activation script to the stdout.
pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(args.config);
    let environment = project.environment_from_name_or_env_var(args.environment)?;

    get_up_to_date_prefix(&environment, args.lock_file_usage.into(), false).await?;

    let output = match args.json {
        true => generate_environment_json(&environment).await?,
        false => generate_activation_script(args.shell, &environment).await?,
    };

    // Print the output - either a JSON object or a shell script
    println!("{}", output);

    Ok(())
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::Platform;
    use rattler_shell::shell::{Bash, CmdExe, Fish, NuShell, PowerShell, Shell, Xonsh, Zsh};

    use super::*;

    #[tokio::test]
    async fn test_shell_hook() {
        let default_shell = rattler_shell::shell::ShellEnum::default();
        let path_var_name = default_shell.path_var(&Platform::current());
        let project = Project::discover().unwrap();
        let environment = project.default_environment();
        let script = generate_activation_script(Some(ShellEnum::Bash(Bash)), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("export {path_var_name}=")));
        assert!(script.contains("export CONDA_PREFIX="));

        let script = generate_activation_script(
            Some(ShellEnum::PowerShell(PowerShell::default())),
            &environment,
        )
        .await
        .unwrap();
        assert!(script.contains(&format!("${{Env:{path_var_name}}}")));
        assert!(script.contains("${Env:CONDA_PREFIX}"));

        let script = generate_activation_script(Some(ShellEnum::Zsh(Zsh)), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("export {path_var_name}=")));
        assert!(script.contains("export CONDA_PREFIX="));

        let script = generate_activation_script(Some(ShellEnum::Fish(Fish)), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("set -gx {path_var_name} ")));
        assert!(script.contains("set -gx CONDA_PREFIX "));

        let script = generate_activation_script(Some(ShellEnum::Xonsh(Xonsh)), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("${path_var_name} = ")));
        assert!(script.contains("$CONDA_PREFIX = "));

        let script = generate_activation_script(Some(ShellEnum::CmdExe(CmdExe)), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("@SET \"{path_var_name}=")));
        assert!(script.contains("@SET \"CONDA_PREFIX="));

        let script = generate_activation_script(Some(ShellEnum::NuShell(NuShell)), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("$env.{path_var_name} = ")));
        assert!(script.contains("$env.CONDA_PREFIX = "));
    }

    #[tokio::test]
    async fn test_environment_json() {
        let default_shell = rattler_shell::shell::ShellEnum::default();
        let path_var_name = default_shell.path_var(&Platform::current());
        let project = Project::discover().unwrap();
        let environment = project.default_environment();
        let json_env = generate_environment_json(&environment).await.unwrap();
        assert!(json_env.contains("\"PIXI_ENVIRONMENT_NAME\":\"default\""));
        assert!(json_env.contains("\"CONDA_PREFIX\":"));
        assert!(json_env.contains(&format!("\"{path_var_name}\":")));
    }
}
