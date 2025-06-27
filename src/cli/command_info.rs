use clap::CommandFactory;
use is_executable::IsExecutable;
use miette::{Context, IntoDiagnostic};
use std::env;
use std::path::PathBuf;

use super::{Command, get_styles};

/// Find a specific external subcommand by name
/// Based on cargo's find_external_subcommand function
pub fn find_external_subcommand(cmd: &str) -> Option<PathBuf> {
    let command_exe = format!("pixi-{}{}", cmd, env::consts::EXE_SUFFIX);
    search_directories().and_then(|dirs| {
        dirs.into_iter()
            .map(|dir| dir.join(&command_exe))
            .find(|path| path.is_executable())
    })
}

/// Execute an external subcommand
pub fn execute_external_command(args: Vec<String>) -> miette::Result<()> {
    // There should be always at least one argument, the command itself.
    // but we dont want to panic on runtime, so we handle it as a error.
    let cmd = args
        .first()
        .ok_or_else(|| miette::miette!("No external subcommand was passed"))?;

    // The rest of the arguments are passed to the external command
    // and we don't mind if there are no additional arguments.
    let cmd_args = &args[1..];

    if let Some(path) = find_external_subcommand(cmd) {
        // ignore any ctrl-c signals
        ctrlc::set_handler(move || {})
            .into_diagnostic()
            .wrap_err("Couldn't set the ctrl-c handler")?;

        let mut command = std::process::Command::new(&path);
        command.args(cmd_args);

        imp::execute_command(command)?;

        Ok(())
    } else {
        // build the error message
        // using the same style as clap's derived error messages
        let styles = get_styles();

        Command::command()
            .styles(styles)
            .error(
                clap::error::ErrorKind::InvalidSubcommand,
                format!("No such command: `pixi {}`", cmd),
            )
            .exit();
    }
}

/// Get directories to search for external commands (pixi extensions)
fn search_directories() -> Option<Vec<PathBuf>> {
    // Right now, we only search the PATH environment variable.
    // In the future, we might want to pixi global directories.
    env::var_os("PATH").map(|paths| env::split_paths(&paths).collect())
}

#[cfg(target_family = "unix")]
mod imp {
    use std::os::unix::process::CommandExt;

    pub(crate) fn execute_command(mut cmd: std::process::Command) -> miette::Result<()> {
        let err = cmd.exec();
        // if calling exec fails, we error out
        // otherwise, the child process replaces the current process
        // and we don't reach this point
        Err(miette::miette!(
            "Failed to execute command '{}': {}",
            cmd.get_program().to_string_lossy(),
            err
        ))
    }
}

#[cfg(target_family = "windows")]
mod imp {
    use miette::{Context, IntoDiagnostic};

    /// On windows, we will rely on spawning the child process
    /// using `CreateProcess``
    /// and waiting for it to complete, since we cannot use `exec`.
    pub(crate) fn execute_command(mut cmd: std::process::Command) -> miette::Result<()> {
        let mut child = cmd.spawn().into_diagnostic().wrap_err(format!(
            "Couldn't spawn the child process {}",
            cmd.get_program().to_string_lossy()
        ))?;

        // Wait for the child process to complete
        let status = child.wait().into_diagnostic().wrap_err(format!(
            "Couldn't wait for the child process {}",
            cmd.get_program().to_string_lossy()
        ))?;

        // Exit with the same status code as the child process
        std::process::exit(status.code().unwrap_or(1));
    }
}
