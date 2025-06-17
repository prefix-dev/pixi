use std::collections::BTreeMap;
use std::path::PathBuf;
use std::{env, fs};

/// Information about a command.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
pub enum CommandInfo {
    /// A built-in command (internal to pixi)
    BuiltIn { about: Option<String> },
    /// An external command (pixi-* executable)
    External { path: PathBuf },
    /// An alias to another command
    Alias { target: Vec<String> },
}

/// Check if a command is a built-in command
/// We just need to know if it exists, not storing function pointers
pub fn builtin_exec(cmd: &str) -> Option<()> {
    match cmd {
        "add" | "auth" | "build" | "clean" | "completion" | "config" | "exec" | "global"
        | "info" | "init" | "install" | "list" | "lock" | "reinstall" | "remove" | "run"
        | "search" | "self-update" | "shell" | "shell-hook" | "task" | "tree" | "update"
        | "upgrade" | "upload" | "workspace" => Some(()),
        _ => None,
    }
}

/// Find external pixi commands in the system PATH
/// Based on cargo's third_party_subcommands function
pub fn find_external_commands() -> BTreeMap<String, CommandInfo> {
    let prefix = "pixi-";
    let suffix = env::consts::EXE_SUFFIX;
    let mut commands = BTreeMap::new();

    for dir in search_directories() {
        #[allow(clippy::disallowed_methods)]
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            _ => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Some(filename) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };

            let Some(name) = filename
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(suffix))
            else {
                continue;
            };

            if is_executable(&path) {
                commands.insert(
                    name.to_string(),
                    CommandInfo::External { path: path.clone() },
                );
            }
        }
    }
    commands
}

/// Find a specific external subcommand by name
/// Based on cargo's find_external_subcommand function
pub fn find_external_subcommand(cmd: &str) -> Option<PathBuf> {
    let command_exe = format!("pixi-{}{}", cmd, env::consts::EXE_SUFFIX);
    search_directories()
        .iter()
        .map(|dir| dir.join(&command_exe))
        .find(is_executable)
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

/// List all available commands (built-in and external)
pub fn list_commands() -> BTreeMap<String, CommandInfo> {
    let mut commands = BTreeMap::new();

    // First add external commands
    commands.extend(find_external_commands());

    // Add built-in commands (these will override any external commands with same name)
    let builtins = [
        ("add", "Add dependencies to the project"),
        ("auth", "Login to prefix.dev or anaconda.org servers"),
        ("build", "Build a package"),
        ("clean", "Clean up the environment"),
        ("completion", "Generate shell completion scripts"),
        ("config", "Configuration management"),
        ("exec", "Execute a command in the environment"),
        ("global", "Global package management"),
        ("info", "Show information about the project"),
        ("init", "Initialize a new project"),
        ("install", "Install the environment"),
        ("list", "List packages in the environment"),
        ("lock", "Update the lockfile"),
        ("reinstall", "Reinstall the environment"),
        ("remove", "Remove dependencies from the project"),
        ("run", "Run a task or command"),
        ("search", "Search for packages"),
        ("self-update", "Update pixi itself"),
        ("shell", "Start a shell in the environment"),
        ("shell-hook", "Generate shell hook scripts"),
        ("task", "Manage tasks"),
        ("tree", "Show dependency tree"),
        ("update", "Update dependencies"),
        ("upgrade", "Upgrade dependencies"),
        ("upload", "Upload a package"),
        ("workspace", "Manage workspace configuration"),
    ];

    for (name, about) in builtins {
        commands.insert(
            name.to_string(),
            CommandInfo::BuiltIn {
                about: Some(about.to_string()),
            },
        );
    }

    commands
}

#[cfg(test)]
#[test]
fn test_command_info() {
    println!("=== Testing Command Info Infrastructure ===");

    let commands = list_commands();
    println!("Found {} commands:", commands.len());

    for (name, info) in &commands {
        match info {
            CommandInfo::BuiltIn { about } => {
                println!(
                    "  {} (built-in): {}",
                    name,
                    about.as_deref().unwrap_or("No description")
                );
            }
            CommandInfo::External { path } => {
                println!("  {} (external): {}", name, path.display());
            }
            CommandInfo::Alias { target } => {
                println!("  {} (alias): -> {}", name, target.join(" "));
            }
        }
    }

    // Test builtin_exec function
    println!("\n=== Testing builtin_exec function ===");
    let test_commands = ["add", "run", "nonexistent"];
    for cmd in test_commands {
        match builtin_exec(cmd) {
            Some(_) => println!("  {}: Found execution function", cmd),
            None => println!(
                "  {}: No execution function (expected for external commands)",
                cmd
            ),
        }
    }
}
