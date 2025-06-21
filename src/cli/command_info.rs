use is_executable::IsExecutable;
use itertools::Itertools;
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
        // Command not found, try to find similar commands
        let suggestions = find_similar_commands(cmd);
        let help_message = if suggestions.is_empty() {
            "help: view all installed commands with `pixi --list`".to_string()
        } else {
            format!(
                "help: view all installed commands with `pixi --list`\n\nDid you mean '{}'?",
                suggestions.join("', '")
            )
        };

        Err(miette::miette!(
            "No such command: `pixi {}`\n\n{}",
            cmd,
            help_message
        ))
    }
}

///fuzzy matching (built-in + external)
fn find_similar_commands(cmd: &str) -> Vec<String> {
    let mut all_commands = Vec::new();

    //built-in commands
    let builtin_commands = vec![
        "add",
        "auth",
        "build",
        "clean",
        "completion",
        "config",
        "exec",
        "global",
        "info",
        "init",
        "install",
        "list",
        "lock",
        "reinstall",
        "remove",
        "run",
        "search",
        "self-update",
        "shell",
        "shell-hook",
        "task",
        "tree",
        "update",
        "upgrade",
        "upload",
        "workspace",
        // Include visible aliases
        "a",
        "x",
        "g",
        "i",
        "ls",
        "rm",
        "r",
        "s",
        "t",
    ];
    all_commands.extend(builtin_commands.iter().map(|s| s.to_string()));

    // Add external commands by discovering them
    if let Ok(external_commands) = find_external_commands() {
        // Strip "pixi-" prefix from external commands
        let external_names: Vec<String> = external_commands
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_prefix("pixi-"))
                    .map(|name| name.to_string())
            })
            .collect();
        all_commands.extend(external_names);
    }

    // Find similar commands using Jaro similarity
    all_commands
        .iter()
        .filter_map(|command| {
            let distance = strsim::jaro(cmd, command);
            if distance > 0.6 {
                Some((command.clone(), distance))
            } else {
                None
            }
        })
        .sorted_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal))
        .take(3) // Show top 3 suggestions
        .map(|(command, _)| command)
        .collect()
}

/// Find all external commands available in PATH
fn find_external_commands() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut external_commands = Vec::new();
    let prefix = "pixi-";

    for dir in search_directories() {
        if let Ok(entries) = fs_err::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if filename.starts_with(prefix) && path.is_executable() {
                        external_commands.push(path);
                    }
                }
            }
        }
    }

    Ok(external_commands)
}

/// Get directories to search for external commands
fn search_directories() -> Vec<PathBuf> {
    if let Some(val) = env::var_os("PATH") {
        env::split_paths(&val).collect()
    } else {
        vec![]
    }
}
