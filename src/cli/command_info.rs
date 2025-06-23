use is_executable::IsExecutable;
use std::env;
use std::path::PathBuf;

/// Find a specific external subcommand by name
/// Based on cargo's find_external_subcommand function
pub fn find_external_subcommand(cmd: &str) -> Option<PathBuf> {
    let command_exe = format!("pixi-{}{}", cmd, env::consts::EXE_SUFFIX);
    search_directories()
        .iter()
        .map(|dir| dir.join(&command_exe))
        .find(|path| path.is_executable())
}

/// Execute an external subcommand
pub fn execute_external_command(args: Vec<String>) -> miette::Result<()> {
    if args.is_empty() {
        return Err(miette::miette!("No command provided"));
    }

    let cmd = &args[0];
    let cmd_args = &args[1..];

    if let Some(path) = find_external_subcommand(cmd) {
        // Execution
        let mut command = std::process::Command::new(&path);
        command.args(cmd_args);

        // Set environment variables that extensions might need
        if let Ok(current_dir) = env::current_dir() {
            command.env("PIXI_PROJECT_ROOT", current_dir);
        }

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
        // Command not found
        Err(miette::miette!(
            "No such command: `pixi {}`\n\nhelp: view all installed commands with `pixi --list`",
            cmd
        ))
    }
}

/// Get directories to search for external commands
fn search_directories() -> Vec<PathBuf> {
    if let Some(val) = env::var_os("PATH") {
        env::split_paths(&val).collect()
    } else {
        vec![]
    }
}
