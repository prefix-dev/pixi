use crate::Project;
use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::shell::{PowerShell, Shell, ShellEnum, ShellScript};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

#[cfg(target_family = "unix")]
use crate::unix::PtySession;

use crate::environment::get_up_to_date_prefix;
use crate::project::environment::get_metadata_env;
#[cfg(target_family = "windows")]
use rattler_shell::shell::CmdExe;

use super::run::run_activation_async;

/// Start a shell in the pixi environment of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,
}

fn start_powershell(
    pwsh: PowerShell,
    env: &HashMap<String, String>,
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

    let mut contents = shell_script.contents;
    // TODO: build a better prompt
    contents.push_str("\nfunction prompt {\"PS pixi> \"}");
    temp_file.write_all(contents.as_bytes()).into_diagnostic()?;
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
fn start_cmdexe(cmdexe: CmdExe, env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
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

    let mut command = std::process::Command::new(shell.executable());
    command.args(args);

    let mut process = PtySession::new(command).into_diagnostic()?;
    process
        // Space added before `source` to automatically ignore it in history.
        .send_line(&format!(" source {}", temp_file.path().display()))
        .into_diagnostic()?;

    process.interact().into_diagnostic()
}

/// Starts a UNIX shell.
/// # Arguments
/// - `shell`: The type of shell to start. Must implement the `Shell` and `Copy` traits.
/// - `args`: A vector of arguments to pass to the shell.
/// - `env`: A HashMap containing environment variables to set in the shell.
#[cfg(target_family = "unix")]
async fn start_nu_shell(
    shell: rattler_shell::shell::NuShell,
    env: &HashMap<String, String>,
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
    // Add a custom prompt to the shell
    let mut contents = shell_script.contents;
    contents.push_str("\n$env.PROMPT_COMMAND = { echo \"(pixi) \" }\n");

    temp_file.write_all(contents.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.arg("--execute");
    command.arg(format!("source {}", temp_file.path().display()));

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

/// Determine the environment variables that need to be set in an interactive shell to make it
/// function as if the environment has been activated. This method runs the activation scripts from
/// the environment and stores the environment variables it added, finally it adds environment
/// variables from the project.
pub async fn get_shell_env(project: &Project) -> miette::Result<HashMap<String, String>> {
    // Get the prefix which we can then activate.
    let prefix = get_up_to_date_prefix(project).await?;

    // Get environment variables from the activation
    let activation_env = run_activation_async(project, prefix).await?;

    // Get environment variables from the manifest
    let manifest_env = get_metadata_env(project);

    // Add the conda default env variable so that the existing tools know about the env.
    let mut shell_env = HashMap::new();
    shell_env.insert("CONDA_DEFAULT_ENV".to_string(), project.name().to_string());

    // Construct command environment by concatenating the environments
    Ok(activation_env
        .into_iter()
        .chain(manifest_env.into_iter())
        .chain(shell_env.into_iter())
        .collect())
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    // Get the environment variables we need to set activate the project in the shell.
    let env = get_shell_env(&project).await?;

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();

    #[cfg(target_family = "windows")]
    let res = match interactive_shell {
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, &env),
        ShellEnum::CmdExe(cmdexe) => start_cmdexe(cmdexe, &env),
        ShellEnum::NuShell(nushell) => start_nu_shell(nushell, &env).await,
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell);
        }
    };

    #[cfg(target_family = "unix")]
    let res = match interactive_shell {
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, &env),
        ShellEnum::Bash(bash) => start_unix_shell(bash, vec!["-l", "-i"], &env).await,
        ShellEnum::Zsh(zsh) => start_unix_shell(zsh, vec!["-l", "-i"], &env).await,
        ShellEnum::Fish(fish) => start_unix_shell(fish, vec![], &env).await,
        ShellEnum::Xonsh(xonsh) => start_unix_shell(xonsh, vec![], &env).await,
        ShellEnum::NuShell(nushell) => start_nu_shell(nushell, &env).await,
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
