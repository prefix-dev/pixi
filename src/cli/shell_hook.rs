use std::{collections::HashMap, default::Default};

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::{ConfigCli, ConfigCliActivation, ConfigCliPrompt};
use rattler_lock::LockFile;
use rattler_shell::{
    activation::{ActivationVariables, PathModificationBehavior},
    shell::{Shell, ShellEnum},
};
use serde::Serialize;
use serde_json;

use crate::{
    activation::{get_activator, CurrentEnvVarBehavior},
    cli::cli_config::{PrefixUpdateConfig, WorkspaceConfig},
    environment::get_update_lock_file_and_prefix,
    prompt,
    workspace::{get_activated_environment_variables, Environment, HasWorkspaceRef},
    UpdateLockFileOptions, Workspace, WorkspaceLocator,
};

use super::cli_config::LockFileUpdateConfig;

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
    pub project_config: WorkspaceConfig,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    config: ConfigCli,

    #[clap(flatten)]
    activation_config: ConfigCliActivation,

    /// The environment to activate in the script
    #[arg(long, short)]
    environment: Option<String>,

    /// Emit the environment variables set by running the activation as JSON
    #[clap(long, default_value = "false", conflicts_with = "shell")]
    json: bool,

    #[clap(flatten)]
    prompt_config: ConfigCliPrompt,
}

#[derive(Serialize)]
struct ShellEnv<'a> {
    environment_variables: &'a HashMap<String, String>,
}

/// Generates the activation script.
async fn generate_activation_script(
    shell: Option<ShellEnum>,
    environment: &Environment<'_>,
    project: &Workspace,
) -> miette::Result<String> {
    // Get shell from the arguments or from the current process or use default if
    // all fails
    let shell = shell.unwrap_or_else(|| {
        ShellEnum::from_parent_process()
            .unwrap_or_else(|| ShellEnum::from_env().unwrap_or_default())
    });

    let activator = get_activator(environment, shell.clone()).into_diagnostic()?;

    let path = std::env::var("PATH")
        .ok()
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>());

    // If we are in a conda environment, we need to deactivate it before activating
    // the host / build prefix
    let conda_prefix = std::env::var("CONDA_PREFIX").ok().map(|p| p.into());
    let mut result = activator
        .activation(ActivationVariables {
            conda_prefix,
            path,
            path_modification_behavior: PathModificationBehavior::default(),
        })
        .into_diagnostic()?;

    if project.config().shell.source_completion_scripts() {
        if let Some(completions_dir) = shell.completion_script_location() {
            result
                .script
                .source_completions(&environment.dir().join(completions_dir))
                .into_diagnostic()?;
        }
    }

    let script = result.script.contents().into_diagnostic()?;
    let hook = prompt::shell_hook(&shell).unwrap_or_default().to_owned();

    if project.config().change_ps1() {
        let prompt_name = prompt::prompt_name(project.name(), environment.name());
        let shell_prompt = prompt::shell_prompt(&shell, prompt_name.as_str());
        Ok([script, hook, shell_prompt].join("\n"))
    } else {
        Ok([script, hook].join("\n"))
    }
}

/// Generates a JSON object describing the changes to the shell environment when
/// activating the provided pixi environment.
async fn generate_environment_json(
    environment: &Environment<'_>,
    lock_file: &LockFile,
    force_activate: bool,
    experimental_cache: bool,
) -> miette::Result<String> {
    let environment_variables = get_activated_environment_variables(
        environment.workspace().env_vars(),
        environment,
        CurrentEnvVarBehavior::Exclude,
        Some(lock_file),
        force_activate,
        experimental_cache,
    )
    .await?;

    let shell_env = ShellEnv {
        environment_variables,
    };

    serde_json::to_string(&shell_env).into_diagnostic()
}

/// Prints the activation script to the stdout.
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = args
        .prompt_config
        .merge_config(args.activation_config.into())
        .merge_config(args.config.clone().into());
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.project_config.workspace_locator_start())
        .locate()?
        .with_cli_config(config);

    let environment = workspace.environment_from_name_or_env_var(args.environment)?;

    let (lock_file_data, _prefix) = get_update_lock_file_and_prefix(
        &environment,
        args.prefix_update_config.update_mode(),
        UpdateLockFileOptions {
            lock_file_usage: args.lock_file_update_config.lock_file_usage(),
            no_install: args.prefix_update_config.no_install
                && args.lock_file_update_config.no_lockfile_update,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        },
    )
    .await?;

    let output = match args.json {
        true => {
            generate_environment_json(
                &environment,
                &lock_file_data.lock_file,
                workspace.config().force_activate(),
                workspace.config().experimental_activation_cache_usage(),
            )
            .await?
        }
        // Skipping the activated environment caching for the script.
        // As it can still run scripts.
        false => generate_activation_script(args.shell, &environment, &workspace).await?,
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
        let project = WorkspaceLocator::default().locate().unwrap();
        let environment = project.default_environment();
        let script =
            generate_activation_script(Some(ShellEnum::Bash(Bash)), &environment, &project)
                .await
                .unwrap();
        assert!(script.contains(&format!("export {path_var_name}=")));
        assert!(script.contains("export CONDA_PREFIX="));

        let script = generate_activation_script(
            Some(ShellEnum::PowerShell(PowerShell::default())),
            &environment,
            &project,
        )
        .await
        .unwrap();
        assert!(script.contains(&format!("${{Env:{path_var_name}}}")));
        assert!(script.contains("${Env:CONDA_PREFIX}"));

        let script = generate_activation_script(Some(ShellEnum::Zsh(Zsh)), &environment, &project)
            .await
            .unwrap();
        assert!(script.contains(&format!("export {path_var_name}=")));
        assert!(script.contains("export CONDA_PREFIX="));

        let script =
            generate_activation_script(Some(ShellEnum::Fish(Fish)), &environment, &project)
                .await
                .unwrap();
        assert!(script.contains(&format!("set -gx {path_var_name} ")));
        assert!(script.contains("set -gx CONDA_PREFIX "));

        let script =
            generate_activation_script(Some(ShellEnum::Xonsh(Xonsh)), &environment, &project)
                .await
                .unwrap();
        assert!(script.contains(&format!("${path_var_name} = ")));
        assert!(script.contains("$CONDA_PREFIX = "));

        let script =
            generate_activation_script(Some(ShellEnum::CmdExe(CmdExe)), &environment, &project)
                .await
                .unwrap();
        assert!(script.contains(&format!("@SET \"{path_var_name}=")));
        assert!(script.contains("@SET \"CONDA_PREFIX="));

        let script =
            generate_activation_script(Some(ShellEnum::NuShell(NuShell)), &environment, &project)
                .await
                .unwrap();
        assert!(script.contains(&format!("$env.{path_var_name} = ")));
        assert!(script.contains("$env.CONDA_PREFIX = "));
    }
}
