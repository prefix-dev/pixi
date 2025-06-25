use clap::CommandFactory;
use is_executable::IsExecutable;
use libc::exit;
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
    // if args.is_empty() {
    //     return Err(miette::miette!("No command provided"));
    // }

    let cmd = &args[0];
    let cmd_args = &args[1..];

    if let Some(path) = find_external_subcommand(cmd) {
        // Execution
        let mut command = std::process::Command::new(&path);
        command.args(cmd_args);

        let status = command
            .status()
            .map_err(|e| miette::miette!("Failed to execute external command '{}': {}", cmd, e))?;

        // Exit with the same code as the external command
        if !status.success() {
            if let Some(code) = status.code() {
                std::process::exit(code);
            } else {
                std::process::exit(1);
            }
        }

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
