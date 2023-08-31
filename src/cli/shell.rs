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

#[cfg(target_family = "windows")]
use rattler_shell::shell::CmdExe;

use super::run::get_task_env;

/// Start a shell in the pixi environment of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,
}

fn start_powershell(
    pwsh: PowerShell,
    task_env: &HashMap<String, String>,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(pwsh.clone(), Platform::current());
    for (key, value) in task_env {
        shell_script.set_env_var(key, value);
    }

    let mut contents = shell_script.contents;
    // TODO: build a better prompt
    contents.push_str("\nfunction prompt {\"PS pixi> \"}");
    temp_file.write_all(contents.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(pwsh.executable());
    command.arg("-NoLogo");
    command.arg("-NoExit");
    command.arg("-File");
    command.arg(temp_file.path());
    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

#[cfg(target_family = "windows")]
fn start_cmdexe(cmdexe: CmdExe, task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".cmd")
        .tempfile()
        .into_diagnostic()?;

    // TODO: Should we just execute the activation scripts directly for cmd.exe?
    let mut shell_script = ShellScript::new(cmdexe, Platform::current());
    for (key, value) in task_env {
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

#[cfg(target_family = "unix")]
async fn start_unix_shell<T: Shell + Copy>(
    shell: T,
    args: Vec<&str>,
    task_env: &HashMap<String, String>,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(&format!(".{}", shell.extension()))
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in task_env {
        shell_script.set_env_var(key, value);
    }
    // TODO - make a good hook to get the users PS1 first
    shell_script.set_env_var("PS1", "pixi> ");
    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.args(args);

    let mut process = PtySession::new(command).into_diagnostic()?;
    process
        .send_line(&format!("source {}", temp_file.path().display()))
        .into_diagnostic()?;

    process.interact().into_diagnostic()
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    let task_env = get_task_env(&project).await?;

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();

    #[cfg(target_family = "windows")]
    let res = match interactive_shell {
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, &task_env),
        ShellEnum::CmdExe(cmdexe) => start_cmdexe(cmdexe, &task_env),
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell);
        }
    };

    #[cfg(target_family = "unix")]
    let res = match interactive_shell {
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, &task_env),
        ShellEnum::Bash(bash) => start_unix_shell(bash, vec!["-l", "-i"], &task_env).await,
        ShellEnum::Zsh(zsh) => start_unix_shell(zsh, vec!["-l", "-i"], &task_env).await,
        ShellEnum::Fish(fish) => start_unix_shell(fish, vec![], &task_env).await,
        ShellEnum::Xonsh(xonsh) => start_unix_shell(xonsh, vec![], &task_env).await,
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
