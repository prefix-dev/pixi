use crate::activation::get_activation_env;
use crate::{prompt, Project};
use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::activation::PathModificationBehavior;
use rattler_shell::shell::{PowerShell, Shell, ShellEnum, ShellScript};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

#[cfg(target_family = "unix")]
use crate::unix::PtySession;

use crate::cli::LockFileUsageArgs;
use crate::project::manifest::EnvironmentName;
#[cfg(target_family = "windows")]
use rattler_shell::shell::CmdExe;

/// Start a shell in the pixi environment of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    lock_file_usage: LockFileUsageArgs,

    #[arg(long, short)]
    environment: Option<String>,
}

fn start_powershell(
    pwsh: PowerShell,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(pwsh.clone(), Platform::current());
    for (key, value) in env {
        shell_script.set_env_var(key, value);
    }
    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(pwsh.executable());
    command.arg("-NoLogo");
    command.arg("-NoExit");
    command.arg("-File");
    command.arg(&temp_path);

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

#[cfg(target_family = "windows")]
fn start_cmdexe(
    cmdexe: CmdExe,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".cmd")
        .tempfile()
        .into_diagnostic()?;

    // TODO: Should we just execute the activation scripts directly for cmd.exe?
    let mut shell_script = ShellScript::new(cmdexe, Platform::current());
    for (key, value) in env {
        shell_script.set_env_var(key, value);
    }
    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(cmdexe.executable());
    command.arg("/K");
    command.arg(temp_file.path());

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

/// Starts a UNIX shell.
/// # Arguments
/// - `shell`: The type of shell to start. Must implement the `Shell` and `Copy` traits.
/// - `args`: A vector of arguments to pass to the shell.
/// - `env`: A HashMap containing environment variables to set in the shell.
#[cfg(target_family = "unix")]
async fn start_unix_shell<T: Shell + Copy>(
    shell: T,
    args: Vec<&str>,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", shell.extension()))
        .rand_bytes(3)
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in env {
        shell_script.set_env_var(key, value);
    }

    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.args(args);

    // Space added before `source` to automatically ignore it in history.
    let mut source_command = " ".to_string();
    shell
        .run_script(&mut source_command, temp_file.path())
        .into_diagnostic()?;

    // Remove automatically added `\n`, if for some reason this fails, just ignore.
    let source_command = source_command
        .strip_suffix('\n')
        .unwrap_or(source_command.as_str());

    // Start process and send env activation to the shell.
    let mut process = PtySession::new(command).into_diagnostic()?;
    process.send_line(source_command).into_diagnostic()?;

    process.interact().into_diagnostic()
}

/// Starts a nu shell.
/// # Arguments
/// - `shell`: The Nushell (also contains executable location)
/// - `env`: A HashMap containing environment variables to set in the shell.
async fn start_nu_shell(
    shell: rattler_shell::shell::NuShell,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", shell.extension()))
        .rand_bytes(3)
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in env {
        if key == "PATH" {
            // split path with PATHSEP
            let paths = std::env::split_paths(value).collect::<Vec<_>>();
            shell_script.set_path(&paths, PathModificationBehavior::Replace);
        } else {
            shell_script.set_env_var(key, value);
        }
    }

    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.arg("--execute");
    command.arg(format!("source {}", temp_file.path().display()));

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment_name = args
        .environment
        .map_or_else(|| EnvironmentName::Default, EnvironmentName::Named);
    let environment = project
        .environment(&environment_name)
        .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))?;

    let prompt_name = match environment_name {
        EnvironmentName::Default => project.name().to_string(),
        EnvironmentName::Named(name) => format!("{}:{}", project.name(), name),
    };

    // Get the environment variables we need to set activate the environment in the shell.
    let env = get_activation_env(&environment, args.lock_file_usage.into()).await?;
    tracing::debug!("Pixi environment activation:\n{:?}", env);

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();

    #[cfg(target_family = "windows")]
    let res = match interactive_shell {
        ShellEnum::NuShell(nushell) => {
            start_nu_shell(nushell, &env, prompt::get_nu_prompt(prompt_name.as_str())).await
        }
        ShellEnum::PowerShell(pwsh) => start_powershell(
            pwsh,
            &env,
            prompt::get_powershell_prompt(prompt_name.as_str()),
        ),
        ShellEnum::CmdExe(cmdexe) => {
            start_cmdexe(cmdexe, &env, prompt::get_cmd_prompt(prompt_name.as_str()))
        }
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell);
        }
    };

    #[cfg(target_family = "unix")]
    let res = match interactive_shell {
        ShellEnum::NuShell(nushell) => {
            start_nu_shell(nushell, env, prompt::get_nu_prompt(prompt_name.as_str())).await
        }
        ShellEnum::PowerShell(pwsh) => start_powershell(
            pwsh,
            env,
            prompt::get_powershell_prompt(prompt_name.as_str()),
        ),
        ShellEnum::Bash(bash) => {
            start_unix_shell(
                bash,
                vec!["-l", "-i"],
                env,
                prompt::get_bash_prompt(prompt_name.as_str()),
            )
            .await
        }
        ShellEnum::Zsh(zsh) => {
            start_unix_shell(
                zsh,
                vec!["-l", "-i"],
                env,
                prompt::get_zsh_prompt(prompt_name.as_str()),
            )
            .await
        }
        ShellEnum::Fish(fish) => {
            start_unix_shell(
                fish,
                vec![],
                env,
                prompt::get_fish_prompt(prompt_name.as_str()),
            )
            .await
        }
        ShellEnum::Xonsh(xonsh) => {
            start_unix_shell(xonsh, vec![], env, prompt::get_xonsh_prompt()).await
        }
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell)
        }
    };

    match res {
        Ok(Some(code)) => std::process::exit(code),
        Ok(None) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error starting shell: {}", e);
            std::process::exit(1);
        }
    }
}
