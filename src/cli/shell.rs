use crate::Project;
use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::shell::{
    Bash, CmdExe, Fish, PowerShell, Shell, ShellEnum, ShellScript, Xonsh, Zsh,
};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

#[cfg(target_family = "unix")]
use crate::unix::PtySession;

use super::run::get_task_env;

/// Start a shell in the pixi environment of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,
}

fn start_powershell(task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .into_diagnostic()?;

    let shell = PowerShell::default();
    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in task_env {
        shell_script.set_env_var(key, value);
    }
    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    let mut command = std::process::Command::new("powershell.exe");
    command.arg("-NoLogo");
    command.arg("-NoExit");
    command.arg("-File");
    command.arg(temp_file.path());
    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

fn start_cmdexe(task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".cmd")
        .tempfile()
        .into_diagnostic()?;

    // TODO: Should we just execute the activation scripts directly for cmd.exe?
    let shell = CmdExe::default();
    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in task_env {
        shell_script.set_env_var(key, value);
    }
    temp_file
        .write_all(shell_script.contents.as_bytes())
        .into_diagnostic()?;

    let mut command = std::process::Command::new("cmd.exe");
    command.arg("/K");
    command.arg(temp_file.path());
    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

async fn start_unix_shell<T: Shell + Copy>(
    shell: T,
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
    command.arg("-l");
    command.arg("-i");

    let mut process = PtySession::new(command).into_diagnostic()?;
    process
        .send_line(&format!("source {}", temp_file.path().display()))
        .into_diagnostic()?;

    process.interact().into_diagnostic()
}

async fn start_zsh(task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    start_unix_shell(Zsh::default(), task_env).await
}

async fn start_bash(task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    start_unix_shell(Bash::default(), task_env).await
}

async fn start_fish(task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    start_unix_shell(Fish::default(), task_env).await
}

async fn start_xonsh(task_env: &HashMap<String, String>) -> miette::Result<Option<i32>> {
    start_unix_shell(Xonsh::default(), task_env).await
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    let task_env = get_task_env(&project).await?;

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();

    let res = match interactive_shell {
        ShellEnum::PowerShell(_) => start_powershell(&task_env),
        ShellEnum::CmdExe(_) => start_cmdexe(&task_env),
        ShellEnum::Zsh(_) => start_zsh(&task_env).await,
        ShellEnum::Bash(_) => start_bash(&task_env).await,
        ShellEnum::Fish(_) => start_fish(&task_env).await,
        ShellEnum::Xonsh(_) => start_xonsh(&task_env).await,
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
