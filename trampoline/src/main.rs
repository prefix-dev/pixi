use fs_err::File;
use miette::{Context, IntoDiagnostic};
use pixi_utils::executable_from_path;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::ops::Not;
#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// trampoline configuration folder name
pub const TRAMPOLINE_CONFIGURATION: &str = "trampoline_configuration";

#[derive(Deserialize, Debug)]
struct Metadata {
    exe: String,
    env: HashMap<String, String>,
}

fn read_metadata(current_exe: &Path) -> miette::Result<Metadata> {
    // the metadata file is next to the current executable parent folder,
    // under trampoline_configuration/current_exe_name.json
    if let Some(exe_parent) = current_exe.parent() {
        let metadata_path = exe_parent
            .join(TRAMPOLINE_CONFIGURATION)
            .join(format!("{}.json", executable_from_path(current_exe),));
        let metadata_file = File::open(&metadata_path)
            .into_diagnostic()
            .wrap_err(format!("Couldn't open {:?}", metadata_path))?;
        let metadata: Metadata = serde_json::from_reader(metadata_file).into_diagnostic()?;
        return Ok(metadata);
    }
    miette::bail!(
        "Couldn't get the parent folder of the current executable: {:?}",
        current_exe
    );
}

/// Compute the difference between two PATH variables (the entries split by `;` or `:`)
fn update_path(cached_path: &str) -> String {
    // Get current PATH
    let current_path = std::env::var("PATH").unwrap_or_default();

    // Split paths into vectors using platform-specific delimiter
    let current_paths: Vec<PathBuf> = std::env::split_paths(&current_path).collect();
    let cached_paths: Vec<PathBuf> = std::env::split_paths(cached_path).collect();

    // Stick all new elements in the front of the cached path
    let new_elements = current_paths
        .iter()
        .filter(|p| cached_paths.contains(p).not());

    // Join the new elements with the current path
    let new_path = std::env::join_paths(new_elements.chain(cached_paths.iter()))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(cached_path.to_string());

    new_path
}

fn trampoline() -> miette::Result<()> {
    // Get command-line arguments (excluding the program name)
    let args: Vec<String> = env::args().collect();
    let current_exe = env::current_exe()
        .into_diagnostic()
        .wrap_err("Couldn't get the `env::current_exe`")?;

    // ignore any ctrl-c signals
    ctrlc::set_handler(move || {})
        .into_diagnostic()
        .wrap_err("Couldn't set the ctrl-c handler")?;

    let metadata = read_metadata(&current_exe)?;

    // Create a new Command for the specified executable
    let mut cmd = Command::new(metadata.exe);

    // Set any additional environment variables
    for (key, value) in metadata.env.iter() {
        // Special case for PATH, which needs to be updated with the current PATH elements
        if key.to_uppercase() == "PATH" {
            cmd.env("PATH", update_path(value));
        } else {
            cmd.env(key, value);
        }
    }

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
            .into_diagnostic()
            .wrap_err("Couldn't spawn the child process")?;

        // Wait for the child process to complete
        let status = child
            .wait()
            .into_diagnostic()
            .wrap_err("Couldn't wait for the child process")?;

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
