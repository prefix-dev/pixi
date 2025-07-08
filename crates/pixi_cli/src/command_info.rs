use clap::CommandFactory;
use is_executable::IsExecutable;
use miette::{Context, IntoDiagnostic};
use pixi_config::pixi_home;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;

use super::{Args, Command, get_styles};

/// Get all built-in command names including aliases (discovered dynamically from clap)
fn get_builtin_commands_with_aliases() -> Vec<String> {
    let mut commands = Vec::new();

    for subcommand in Args::command().get_subcommands() {
        // Add main command name
        commands.push(subcommand.get_name().to_string());

        // Add all aliases
        commands.extend(subcommand.get_all_aliases().map(|alias| alias.to_string()));
    }

    commands
}

/// All available commands (built-in + external)
fn get_all_available_commands() -> Vec<String> {
    let mut all_commands = HashSet::new();

    all_commands.extend(get_builtin_commands_with_aliases());

    all_commands.extend(find_external_commands().into_keys());

    all_commands.into_iter().collect()
}

/// Find similar commands using Jaro similarity
fn find_similar_commands(input: &str) -> Vec<String> {
    let available_commands = get_all_available_commands();
    let mut suggestions: Vec<(f64, String)> = Vec::new();
    let threshold = 0.6;

    for command in available_commands {
        let similarity = strsim::jaro(input, &command);
        if similarity > threshold {
            suggestions.push((similarity, command));
        }
    }

    // Sort by similarity (ascending), (most similar at the end)
    suggestions.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    suggestions.into_iter().map(|(_, cmd)| cmd).collect()
}

/// Find all external commands available in PATH
pub(crate) fn find_external_commands() -> HashMap<String, PathBuf> {
    let mut commands = HashMap::new();

    if let Some(dirs) = search_directories() {
        for dir in dirs {
            if let Ok(entries) = fs_err::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        // Check if it's a pixi extension
                        if let Some(cmd_name) = name.strip_prefix("pixi-") {
                            // Remove .exe suffix on Windows
                            let cmd_name = {
                                #[cfg(target_family = "windows")]
                                {
                                    cmd_name
                                        .strip_suffix(env::consts::EXE_SUFFIX)
                                        .unwrap_or(cmd_name)
                                }
                                #[cfg(not(target_family = "windows"))]
                                {
                                    cmd_name
                                }
                            };

                            let path = entry.path();
                            if path.is_executable() {
                                commands.insert(cmd_name.to_string(), path);
                            }
                        }
                    }
                }
            }
        }
    }

    commands
}

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
        // Generate suggestions for similar commands
        let mut suggestions = find_similar_commands(cmd);

        let styles = get_styles();

        // get the styles for invalid and valid commands
        let invalid = styles.get_invalid();
        let tip = styles.get_valid();

        let mut error_msg = format!("unrecognized subcommand '{invalid}{cmd}{invalid:#}'");

        if let Some(most_similar) = suggestions.pop() {
            error_msg.push_str(&format!(
                "\n\n  {tip}tip{tip:#}: a similar subcommand exists: '{tip}{most_similar}{tip:#}'",
            ));
        }

        Command::command()
            .styles(styles)
            .error(clap::error::ErrorKind::InvalidSubcommand, error_msg)
            .exit();
    }
}

/// Get directories to search for external commands (pixi extensions)
fn search_directories() -> Option<Vec<PathBuf>> {
    let mut directories = Vec::new();

    // PATH directories
    if let Some(path_dirs) = env::var_os("PATH") {
        directories.extend(env::split_paths(&path_dirs));
    }

    // pixi global bin directory
    if let Some(pixi_home_path) = pixi_home() {
        let global_bin = pixi_home_path.join("bin");
        if global_bin.exists() {
            directories.push(global_bin);
        }
    }

    if directories.is_empty() {
        None
    } else {
        Some(directories)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tee_suggests_tree() {
        let suggestions = find_similar_commands("tee");
        assert!(suggestions.contains(&"tree".to_string()));
    }
}
