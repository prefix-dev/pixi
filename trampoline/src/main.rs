use fs_err::File;
use anyhow::{Result, Context};
use pixi_exec_utils::executable_from_path;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::ops::Not;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;

// trampoline configuration folder name
pub const TRAMPOLINE_CONFIGURATION: &str = "trampoline_configuration";

#[derive(Deserialize, Debug)]
pub struct Configuration {
    /// Path to the original executable.
    pub exe: PathBuf,
    /// Root path of the original executable that should be prepended to the PATH.
    pub path_diff: String,
    /// Environment variables to be set before executing the original executable.
    pub env: HashMap<String, String>,
}

fn read_configuration(current_exe: &Path) -> Result<Configuration> {
    // the configuration file is next to the current executable parent folder,
    // under trampoline_configuration/current_exe_name.json
    if let Some(exe_parent) = current_exe.parent() {
        let configuration_path = exe_parent
            .join(TRAMPOLINE_CONFIGURATION)
            .join(format!("{}.json", executable_from_path(current_exe)));

        let configuration_file = File::open(&configuration_path)
            .with_context(|| format!("Couldn't open {}", configuration_path.display()))?;

        let configuration: Configuration = serde_json::from_reader(configuration_file)
            .with_context(|| format!("Failed to parse config {}", configuration_path.display()))?;

        return Ok(configuration);
    }

    Err(anyhow::anyhow!(
        "Couldn't get the parent folder of the current executable: {}",
        current_exe.display()
    ))
}

/// Compute the difference between two PATH variables (the entries split by `;` or `:`)
fn setup_path(path_diff: &str) -> Result<String> {
    let current_path = std::env::var("PATH").context("Failed to read PATH env var")?;
    let current_paths = std::env::split_paths(&current_path);
    let path_diffs = std::env::split_paths(path_diff);

    let paths: Vec<PathBuf> = if let Ok(base_path) = std::env::var("PIXI_BASE_PATH") {
        let base_paths: Vec<PathBuf> = env::split_paths(&base_path).collect();
        let new_parts: Vec<PathBuf> = current_paths
            .filter(|current| base_paths.contains(current).not())
            .collect();

        new_parts
            .into_iter()
            .chain(path_diffs)
            .chain(base_paths)
            .collect()
    } else {
        path_diffs.chain(current_paths).collect()
    };

    Ok(std::env::join_paths(paths)
        .context("Failed to join PATH components")?
        .to_string_lossy()
        .to_string())
}

fn trampoline() -> Result<()> {
    // Get command-line arguments (excluding the program name)
    let args: Vec<String> = env::args().collect();
    let current_exe = env::current_exe()
        .context("Couldn't get `env::current_exe`")?;

    // ignore any ctrl-c signals
    ctrlc::set_handler(move || {})
        .context("Couldn't set the ctrl-c handler")?;

    let configuration = read_configuration(&current_exe)?;

    // Create a new Command for the specified executable
    let mut cmd = Command::new(configuration.exe);

    // Set any additional environment variables
    for (key, value) in configuration.env.iter() {
        cmd.env(key, value);
    }

    // Special case for PATH
    cmd.env("PATH", setup_path(&configuration.path_diff)?);

    // Add any additional arguments
    cmd.args(&args[1..]);

    // Configure stdin, stdout, and stderr to use the current process's streams
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    // Spawn the child process
    #[cfg(target_family = "unix")]
    {
        let err = cmd.exec();
        eprintln!("Failed to execute command: {:?}", err);
        std::process::exit(1);
    }

    #[cfg(target_os = "windows")]
    {
        let mut child = cmd
            .spawn()
            .context("Couldn't spawn the child process")?;

        // Wait for the child process to complete
        let status = child
            .wait()
            .context("Couldn't wait for the child process")?;

        // Exit with the same status code as the child process
        std::process::exit(status.code().unwrap_or(1));
    }
}

// Entry point for the trampoline
fn main() {
    if let Err(err) = trampoline() {
        eprintln!("{:?}", err);
        std::process::exit(1);
    }
}
