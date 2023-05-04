use std::io::Write;
use std::path::PathBuf;

use crate::project::Project;
use clap::Parser;
use rattler_conda_types::Platform;

use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell::{Shell, ShellEnum};

/// Adds a dependency to the project
#[derive(Parser, Debug)]
pub struct Args {
    command: String,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::discover()?;

    let shell: ShellEnum = ShellEnum::detect_from_environment()
        .ok_or_else(|| anyhow::anyhow!("could not detect the current shell"))?;

    // write the script to execute the command + activation of the environment
    let prefix = project.root().join(".pax/env");
    let activator = Activator::from_path(&prefix, shell.clone(), Platform::current())?;

    let path = std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .map(|p| PathBuf::from(p))
        .collect::<Vec<_>>();

    let activator_result = activator.activation(ActivationVariables {
        path: Some(path),
        conda_prefix: None,
    })?;

    //
    let mut script = format!("{}\n", activator_result.script.trim());
    shell.run_command(&mut script, [args.command.as_str()])?;

    // Create a temporary file that we can execute
    let mut temp_file = tempfile::Builder::new()
        .suffix(&format!(".{}", shell.extension()))
        .tempfile()?;
    temp_file.write(script.as_bytes())?;

    let mut command = shell
        .create_run_script_command(temp_file.path())
        .spawn()
        .expect("failed to execute process");

    std::process::exit(command.wait().unwrap().code().unwrap_or(1));
}
