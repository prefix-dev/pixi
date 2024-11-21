use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs::File;
use pixi_utils::executable_from_path;
#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use miette::{IntoDiagnostic, Context};


// trampoline configuration folder name
pub const TRAMPOLINE_CONFIGURATION: &str = "trampoline_configuration";

#[derive(Deserialize, Debug)]
struct Metadata {
    exe: String,
    prefix: PathBuf,
    env: HashMap<String, String>,
}

fn read_metadata(current_exe: &Path) -> miette::Result<Metadata> {
    // the metadata file is next to the current executable parent folder,
    // under trampoline_configuration/current_exe_name.json
    if let Some(exe_parent) = current_exe.parent(){
        let metadata_path = exe_parent.join(TRAMPOLINE_CONFIGURATION).join(format!("{}{}", executable_from_path(current_exe), ".json"));
        let metadata_file = File::open(&metadata_path).into_diagnostic().wrap_err(format!("Couldn't open {:?}", metadata_path))?;
        let metadata: Metadata = serde_json::from_reader(metadata_file).into_diagnostic()?;
        return Ok(metadata);
    }
    miette::bail!("Couldn't get the parent folder of the current executable: {:?}", current_exe);
}

fn prepend_path(prefix: &Path) -> miette::Result<String> {
    let path = env::var("PATH").into_diagnostic().wrap_err("Couldn't get 'PATH'")?;

    #[cfg(target_os = "windows")]
    let mut path_entries = vec![
        prefix.to_path_buf(),
        prefix.join("Library/mingw-w64/bin"),
        prefix.join("Library/usr/bin"),
        prefix.join("Library/bin"),
        prefix.join("Scripts"),
        prefix.join("bin"),
    ];

    #[cfg(target_family = "unix")]
    let mut path_entries = vec![prefix.join("bin")];

    let prev_path = env::split_paths(&path).collect::<Vec<_>>();
    let new_path = path_entries.iter().chain(prev_path.iter());

    let new_path = env::join_paths(&new_path)
        .into_diagnostic()
        .wrap_err(format!("Couldn't join PATH's: {:?}", &new_path))?;

    Ok(new_path.to_string_lossy().into_owned())
}

fn trampoline() -> miette::Result<()> {
    // Get command-line arguments (excluding the program name)
    let args: Vec<String> = env::args().collect();
    let current_exe = env::current_exe().into_diagnostic().wrap_err("Couldn't get the `env::current_exe`")?;

    // ignore any ctrl-c signals
    ctrlc::set_handler(move || {}).into_diagnostic().wrap_err("Could not unset the ctrl-c handler")?;

    let metadata = read_metadata(&current_exe)?;

    // Create a new Command for the specified executable
    let mut cmd = Command::new(metadata.exe);

    // Set any additional environment variables
    for (key, value) in metadata.env.iter() {
        cmd.env(key, value);
    }

    // Prepend the specified path to the PATH environment variable
    let new_path = prepend_path(&metadata.path)?;

    // Set the PATH environment variable
    cmd.env("PATH", new_path);

    // Add any additional arguments
    cmd.args(&args[1..]);

    // Configure stdin, stdout, and stderr to use the current process's streams
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    // Spawn the child process
    #[cfg(target_family = "unix")]
    cmd.exec();

    #[cfg(target_os = "windows")]
    {
        let mut child = cmd.spawn().into_diagnostic().wrap_err("Couldn't spawn the child process")?;

        // Wait for the child process to complete
        let status = child.wait().into_diagnostic().wrap_err("Couldn't wait for the child process")?;

        // Exit with the same status code as the child process
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

// Entry point for the trampoline
fn main() {
    if let Err(err) = trampoline() {
        eprintln!("{:?}", err);
        std::process::exit(1);
    }
}
