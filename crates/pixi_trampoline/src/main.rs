use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs::File;
#[cfg(target_family = "unix")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};


// trampoline configuration folder name
pub const TRAMPOLINE_CONFIGURATION: &str = "trampoline_configuration";

#[derive(Deserialize, Debug)]
struct Metadata {
    exe: String,
    path: String,
    env: HashMap<String, String>,
}

fn read_metadata(current_exe: &Path) -> Metadata {
    // the metadata file is next to the current executable parent folder,
    // under trampoline_configuration/current_exe_name.json
    let exe_parent = current_exe.parent().expect("should have a parent");
    let exe_name = current_exe.file_stem().expect("should have a file name");
    let metadata_path = exe_parent.join(TRAMPOLINE_CONFIGURATION).join(format!("{}{}", exe_name.to_string_lossy(), ".json"));
    let metadata_file = File::open(metadata_path).unwrap();
    let metadata: Metadata = serde_json::from_reader(metadata_file).unwrap();
    metadata
}

fn prepend_path(extra_path: &str) -> String {
    let path = env::var("PATH").unwrap();
    let mut split_path = env::split_paths(&path).collect::<Vec<_>>();
    split_path.insert(0, PathBuf::from(extra_path));
    let new_path = env::join_paths(split_path).unwrap();
    new_path.to_string_lossy().into_owned()
}

fn main() -> () {
    // Get command-line arguments (excluding the program name)
    let args: Vec<String> = env::args().collect();
    let current_exe = env::current_exe().expect("Failed to get current executable path");

    // ignore any ctrl-c signals
    ctrlc::set_handler(move || {}).expect("Error setting Ctrl-C handler");

    let metadata = read_metadata(&current_exe);

    // Create a new Command for the specified executable
    let mut cmd = Command::new(metadata.exe);

    let new_path = prepend_path(&metadata.path);

    // Set the PATH environment variable
    cmd.env("PATH", new_path);

    // Set any additional environment variables
    for (key, value) in metadata.env.iter() {
        cmd.env(key, value);
    }

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
        let mut child = cmd.spawn().expect("process spawn should succeed");

        // Wait for the child process to complete
        let status = child.wait().expect("failed to wait on child");

        // Exit with the same status code as the child process
        std::process::exit(status.code().unwrap_or(1));
    }
}
