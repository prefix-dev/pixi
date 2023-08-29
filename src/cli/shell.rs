use crate::Project;
use clap::Parser;
use miette::{bail, IntoDiagnostic};
use rattler_conda_types::Platform;
use rattler_shell::shell::{CmdExe, PowerShell, Shell, ShellEnum, ShellScript, Zsh};
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

fn start_powershell(task_env: &HashMap<String, String>) -> miette::Result<()> {
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
    command.spawn().into_diagnostic()?;
    Ok(())
}

fn start_cmdexe(task_env: &HashMap<String, String>) -> miette::Result<()> {
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
    command.spawn().into_diagnostic()?;
    Ok(())
}

async fn start_zsh(task_env: &HashMap<String, String>) -> miette::Result<()> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".zsh")
        .tempfile()
        .into_diagnostic()?;

    let shell = Zsh::default();
    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in task_env {
        shell_script.set_env_var(key, value);
    }
    shell_script.set_env_var("PS1", "pixi>");
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
    process.interact().into_diagnostic()?;
    println!("Its time to quit.");
    Ok(())
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
        _ => bail!("Unsupported shell: {:?}", interactive_shell),
    };
    println!("res: {:?}", res);

    // // Generate a temporary file with the script to execute. This includes the activation of the
    // // environment.
    // let mut script = format!("{}\n", activator_result.script.trim());

    // // Add meta data env variables to help user interact with there configuration.
    // add_metadata_as_env_vars(&mut script, &shell, &project)?;

    // // Add the conda default env variable so that the tools that use this know it exists.
    // shell
    //     .set_env_var(&mut script, "CONDA_DEFAULT_ENV", project.name())
    //     .into_diagnostic()?;

    // // Start the shell as the last part of the activation script based on the default shell.
    // script.push_str(interactive_shell.executable());

    // // Write the contents of the script to a temporary file that we can execute with the shell.
    // let mut temp_file = tempfile::Builder::new()
    //     .suffix(&format!(".{}", shell.extension()))
    //     .tempfile()
    //     .into_diagnostic()?;
    // std::io::Write::write_all(&mut temp_file, script.as_bytes()).into_diagnostic()?;

    // // Execute the script with the shell
    // let mut command = shell
    //     .create_run_script_command(temp_file.path())
    //     .spawn()
    //     .expect("failed to execute process");

    // std::process::exit(command.wait().into_diagnostic()?.code().unwrap_or(1));

    Ok(())
}
