use std::{io::Write, path::PathBuf};

use crate::Project;
use clap::Parser;
use rattler_conda_types::Platform;

use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::{Shell, ShellEnum},
};

/// Runs command in project.
#[derive(Parser, Debug)]
#[clap(trailing_var_arg=true)]
pub struct Args {
    command: Vec<String>,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::discover()?;

    // Determine the current shell
    let shell: ShellEnum = ShellEnum::detect_from_environment()
        .ok_or_else(|| anyhow::anyhow!("could not detect the current shell"))?;

    // Construct an activator so we can run commands from the environment
    let prefix = project.root().join(".pax/env");
    let activator = Activator::from_path(&prefix, shell.clone(), Platform::current())?;

    let path = std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    let activator_result = activator.activation(ActivationVariables {
        path: Some(path),
        conda_prefix: None,
    })?;

    // Generate a temporary file with the script to execute. This includes the activation of the
    // environment.
    let mut script = format!("{}\n", activator_result.script.trim());
    shell.run_command(&mut script, args.command.iter().map(|s| s.as_str()))?;

    let mut temp_file = tempfile::Builder::new()
        .suffix(&format!(".{}", shell.extension()))
        .tempfile()?;
    temp_file.write_all(script.as_bytes())?;

    // Execute the script with the shell
    let mut command = shell
        .create_run_script_command(temp_file.path())
        .spawn()
        .expect("failed to execute process");

    std::process::exit(command.wait()?.code().unwrap_or(1));
}
