use std::path::PathBuf;
use std::{env, fs};

/// Find a specific external subcommand by name
/// Based on cargo's find_external_subcommand function
pub fn find_external_subcommand(cmd: &str) -> Option<PathBuf> {
    let command_exe = format!("pixi-{}{}", cmd, env::consts::EXE_SUFFIX);
    search_directories()
        .iter()
        .map(|dir| dir.join(&command_exe))
        .find(is_executable)
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
        // TODO: Add "did you mean" suggestions here
        Err(miette::miette!(
            "No such command: `pixi {}`\n\nhelp: view all installed commands with `pixi --list`",
            cmd
        ))
    }
}

/// Get directories to search for external commands
fn search_directories() -> Vec<PathBuf> {
    let mut path_dirs = if let Some(val) = env::var_os("PATH") {
        env::split_paths(&val).collect()
    } else {
        vec![]
    };

    // Add home_bin if not already in PATH, following cargo's pattern
    if let Ok(home) = env::var("HOME") {
        let home_bin = PathBuf::from(&home).join("bin");

        // Add them if not already in PATH
        if !path_dirs.iter().any(|p| p == &home_bin) {
            path_dirs.insert(0, home_bin);
        }
    }

    path_dirs
}

/// Check if a file is executable
#[cfg(unix)]
fn is_executable(path: &PathBuf) -> bool {
    use std::os::unix::prelude::*;
    #[allow(clippy::disallowed_methods)]
    let result = fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false);
    result
}

#[cfg(windows)]
fn is_executable(path: &PathBuf) -> bool {
    path.is_file()
}

#[cfg(test)]
#[test]
fn test_external_command_discovery() {
    // Test that search_directories includes PATH
    let dirs = search_directories();
    assert!(!dirs.is_empty(), "Should find at least some directories");

    // Test command name formatting
    let cmd = "deploy";
    let expected = format!("pixi-{}{}", cmd, env::consts::EXE_SUFFIX);

    // Mock test - we don't expect to find this command
    let result = find_external_subcommand(cmd);
    // This should be None unless someone has a pixi-deploy in their PATH
    println!("Searched for {}, found: {:?}", expected, result);
}
