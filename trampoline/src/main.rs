use fs_err::File;
use miette::{Context, IntoDiagnostic};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::ops::Not;
#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
mod executable_from_path;
use executable_from_path::executable_from_path;

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

fn read_configuration(current_exe: &Path) -> miette::Result<Configuration> {
    // the configuration file is next to the current executable parent folder,
    // under trampoline_configuration/current_exe_name.json
    if let Some(exe_parent) = current_exe.parent() {
        let configuration_path = exe_parent
            .join(TRAMPOLINE_CONFIGURATION)
            .join(format!("{}.json", executable_from_path(current_exe),));
        let configuration_file = File::open(&configuration_path)
            .map(std::io::BufReader::new)
            .into_diagnostic()
            .wrap_err(format!("Couldn't open {:?}", configuration_path))?;
        let configuration: Configuration =
            serde_json::from_reader(configuration_file).into_diagnostic()?;
        return Ok(configuration);
    }
    miette::bail!(
        "Couldn't get the parent folder of the current executable: {:?}",
        current_exe
    );
}

/// Compute the difference between two PATH variables (the entries split by `;` or `:`)
fn setup_path(path_diff: &str) -> miette::Result<String> {
    // PATH may be unset when the trampoline is invoked by a program that
    // clears the child environment (e.g. podman calling the OCI runtime).
    // Treat a missing PATH the same as an empty one instead of erroring out.
    let current_path = std::env::var_os("PATH");
    let base_path = std::env::var_os("PIXI_BASE_PATH");
    compose_path(path_diff, current_path.as_deref(), base_path.as_deref())
}

fn compose_path(
    path_diff: &str,
    current_path: Option<&std::ffi::OsStr>,
    base_path: Option<&std::ffi::OsStr>,
) -> miette::Result<String> {
    let current_paths = current_path
        .into_iter()
        .flat_map(std::env::split_paths);
    let path_diffs = std::env::split_paths(path_diff);

    let paths: Vec<PathBuf> = if let Some(base_path) = base_path {
        let base_paths: Vec<PathBuf> = std::env::split_paths(base_path).collect();
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

    std::env::join_paths(paths)
        .into_diagnostic()
        .map(|p| p.to_string_lossy().to_string())
}

fn trampoline() -> miette::Result<()> {
    // Get command-line arguments (excluding the program name)
    let args: Vec<String> = env::args().collect();
    let current_exe = env::current_exe()
        .into_diagnostic()
        .wrap_err("Couldn't get the `env::current_exe`")?;

    // Resolve symlinks so that config lookup works even when the trampoline is
    // invoked via a symlink from a different directory. On Linux,
    // `current_exe()` reads /proc/self/exe which auto-resolves symlinks, but
    // on macOS (_NSGetExecutablePath) returns the symlink path rather than the target.
    // https://doc.rust-lang.org/std/env/fn.current_exe.html#platform-specific-behavior
    // On Windows, we intentionally do not resolve symlinks because the trampoline
    // may be invoked via a symlink from a different directory.
    #[cfg(not(windows))]
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);

    // ignore any ctrl-c signals
    ctrlc::set_handler(move || {})
        .into_diagnostic()
        .wrap_err("Couldn't set the ctrl-c handler")?;

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

#[cfg(test)]
mod tests {
    use super::compose_path;
    use std::ffi::OsStr;

    #[test]
    fn missing_path_uses_path_diff_only() {
        let result = compose_path("/opt/env/bin", None, None).unwrap();
        assert_eq!(result, "/opt/env/bin");
    }

    #[test]
    fn existing_path_is_prepended_with_path_diff() {
        let result =
            compose_path("/opt/env/bin", Some(OsStr::new("/usr/bin:/bin")), None).unwrap();
        assert_eq!(result, "/opt/env/bin:/usr/bin:/bin");
    }

    #[test]
    fn base_path_entries_are_moved_to_end() {
        let result = compose_path(
            "/opt/env/bin",
            Some(OsStr::new("/custom:/usr/bin:/bin")),
            Some(OsStr::new("/usr/bin:/bin")),
        )
        .unwrap();
        assert_eq!(result, "/custom:/opt/env/bin:/usr/bin:/bin");
    }
}
