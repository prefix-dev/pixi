use std::{collections::HashMap, default::Default};

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::{ConfigCliActivation, ConfigCliPrompt};
use pixi_manifest::EnvironmentName;
use rattler_lock::LockFile;
use rattler_shell::{
    activation::{ActivationVariables, PathModificationBehavior},
    shell::ShellEnum,
};
use serde::Serialize;
use serde_json;

use crate::activation::CurrentEnvVarBehavior;
use crate::environment::get_update_lock_file_and_prefix;
use crate::{
    activation::get_activator,
    cli::cli_config::{PrefixUpdateConfig, ProjectConfig},
    project::{Environment, HasProjectRef},
    prompt, Project, UpdateLockFileOptions,
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
    pub prefix_update_config: PrefixUpdateConfig,

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

fn get_shell(shell: Option<ShellEnum>) -> ShellEnum {
    // Get shell from the arguments or from the current process or use default if
    // all fails
    shell.unwrap_or_else(|| {
        ShellEnum::from_parent_process()
            .unwrap_or_else(|| ShellEnum::from_env().unwrap_or_default())
    })
}

/// Generates the activation script.
async fn generate_activation_script(
    shell: ShellEnum,
    environment: &Environment<'_>,
) -> miette::Result<String> {
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
async fn generate_environment_json(
    environment: &Environment<'_>,
    lock_file: &LockFile,
    force_activate: bool,
    experimental_cache: bool,
) -> miette::Result<String> {
    let environment_variables = environment
        .project()
        .get_activated_environment_variables(
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
        .merge_config(args.prefix_update_config.config.clone().into());
    let project = Project::load_or_else_discover(args.project_config.manifest_path.as_deref())?
        .with_cli_config(config);
    let environment = project.environment_from_name_or_env_var(args.environment)?;

    let (lock_file_data, _prefix) = get_update_lock_file_and_prefix(
        &environment,
        args.prefix_update_config.update_mode(),
        UpdateLockFileOptions {
            lock_file_usage: args.prefix_update_config.lock_file_usage(),
            no_install: args.prefix_update_config.no_install(),
            max_concurrent_solves: project.config().max_concurrent_solves(),
        },
    )
    .await?;

    let output = match args.json {
        true => {
            generate_environment_json(
                &environment,
                &lock_file_data.lock_file,
                project.config().force_activate(),
                project.config().experimental_activation_cache_usage(),
            )
            .await?
        }
        // Skipping the activated environment caching for the script.
        // As it can still run scripts.
        false => {
            let shell = get_shell(args.shell);
            let script = generate_activation_script(shell.clone(), &environment).await?;
            let in_shell = match std::env::var("PIXI_IN_SHELL") {
                Ok(val) => val != "0" && !val.is_empty(),
                Err(_) => false,
            };
            if project.config().change_ps1() && !in_shell {
                let prompt_name = match environment.name() {
                    EnvironmentName::Default => project.name().to_string(),
                    EnvironmentName::Named(name) => format!("{}:{}", project.name(), name),
                };
                let prompt = match shell {
                    ShellEnum::NuShell(_) => prompt::get_nu_prompt(prompt_name.as_str()),
                    ShellEnum::PowerShell(_) => prompt::get_powershell_prompt(prompt_name.as_str()),
                    ShellEnum::Bash(_) => prompt::get_posix_prompt(prompt_name.as_str()),
                    ShellEnum::Zsh(_) => prompt::get_posix_prompt(prompt_name.as_str()),
                    ShellEnum::Fish(_) => prompt::get_fish_prompt(prompt_name.as_str()),
                    ShellEnum::Xonsh(_) => prompt::get_xonsh_prompt(),
                    ShellEnum::CmdExe(_) => prompt::get_cmd_prompt(prompt_name.as_str()),
                };
                format!("{}\n{}", script, prompt)
            } else {
                script
            }
        }
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
        let script = generate_activation_script(ShellEnum::Bash(Bash), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("export {path_var_name}=")));
        assert!(script.contains("export CONDA_PREFIX="));

        let script =
            generate_activation_script(ShellEnum::PowerShell(PowerShell::default()), &environment)
                .await
                .unwrap();
        assert!(script.contains(&format!("${{Env:{path_var_name}}}")));
        assert!(script.contains("${Env:CONDA_PREFIX}"));

        let script = generate_activation_script(ShellEnum::Zsh(Zsh), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("export {path_var_name}=")));
        assert!(script.contains("export CONDA_PREFIX="));

        let script = generate_activation_script(ShellEnum::Fish(Fish), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("set -gx {path_var_name} ")));
        assert!(script.contains("set -gx CONDA_PREFIX "));

        let script = generate_activation_script(ShellEnum::Xonsh(Xonsh), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("${path_var_name} = ")));
        assert!(script.contains("$CONDA_PREFIX = "));

        let script = generate_activation_script(ShellEnum::CmdExe(CmdExe), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("@SET \"{path_var_name}=")));
        assert!(script.contains("@SET \"CONDA_PREFIX="));

        let script = generate_activation_script(ShellEnum::NuShell(NuShell), &environment)
            .await
            .unwrap();
        assert!(script.contains(&format!("$env.{path_var_name} = ")));
        assert!(script.contains("$env.CONDA_PREFIX = "));
    }
}
