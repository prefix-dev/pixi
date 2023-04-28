use std::path::PathBuf;
use std::process::Command;

use crate::project::Project;
use clap::Parser;
use rattler_conda_types::Platform;

use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell::{Bash, Shell};

/// Adds a dependency to the project
#[derive(Parser, Debug)]
pub struct Args {
    command: String,
}

// TODO: I dont like this command, if it is at all possible it would be so much better when this
//  command is run when needed. E.g. have a cheap way to determine if the environment is up-to-date,
//  if not, update it.
pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::discover()?;
    let commands = project.commands()?;

    let command_str = commands
        .get(&args.command)
        .ok_or_else(|| anyhow::anyhow!("command not found"))?;

    // write the script to execute the command + activation of the environment
    let prefix = PathBuf::from("./.prefix");
    let activator = Activator::from_path(&prefix, Bash, Platform::current())?;

    let path = std::env::split_paths(&std::env::var("PATH").unwrap_or_default())
        .map(|p| PathBuf::from(p))
        .collect::<Vec<_>>();

    let activator_result = activator.activation(ActivationVariables {
        path: Some(path),
        conda_prefix: None,
    })?;

    let mut script = activator_result.script;

    script.push_str(&format!("\n{}\n", command_str));

    let mut command = Command::new("bash")
        .arg("-c")
        .arg(script)
        .spawn()
        .expect("failed to execute process");

    std::process::exit(command.wait().unwrap().code().unwrap_or(1));
}
