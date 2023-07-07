use anyhow::Context;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::string::String;

use clap::Parser;
use deno_task_shell::parser::SequentialList;
use deno_task_shell::{execute_with_pipes, get_output_writer_and_handle, pipe, ShellState};
use itertools::Itertools;
use rattler_conda_types::Platform;

use crate::prefix::Prefix;
use crate::progress::await_in_progress;
use crate::project::environment::get_metadata_env;
use crate::{
    command::{CmdArgs, Command, ProcessCmd},
    environment::get_up_to_date_prefix,
    Project,
};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehaviour},
    shell::ShellEnum,
};

// Run output which includes the information gotten from the deno task shell run.
#[derive(Default)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs command in project.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// The command you want to run in the projects environment.
    pub command: Vec<String>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

pub fn order_commands(
    commands: Vec<String>,
    project: &Project,
) -> anyhow::Result<VecDeque<(Command, Vec<String>)>> {
    let command: Vec<_> = commands.iter().map(|c| c.to_string()).collect();

    // Find the command in the project.
    let (command_name, command, additional_args) = command
        .first()
        .and_then(|cmd_name| {
            project.command_opt(cmd_name).map(|cmd| {
                (
                    Some(cmd_name.clone()),
                    cmd.clone(),
                    command[1..].iter().cloned().collect_vec(),
                )
            })
        })
        .unwrap_or_else(|| {
            (
                None,
                Command::Process(ProcessCmd {
                    cmd: CmdArgs::Multiple(commands),
                    depends_on: vec![],
                }),
                Vec::new(),
            )
        });

    // Perform post order traversal of the commands and their `depends_on` to make sure they are
    // executed in the right order.
    let mut s1 = VecDeque::new();
    let mut s2 = VecDeque::new();
    let mut added = HashSet::new();

    // Add the command specified on the command line first
    s1.push_back((command, additional_args));
    if let Some(command_name) = command_name {
        added.insert(command_name);
    }

    while let Some((command, additional_args)) = s1.pop_back() {
        // Get the dependencies of the command
        let depends_on = match &command {
            Command::Process(process) => process.depends_on.as_slice(),
            Command::Alias(alias) => &alias.depends_on,
            _ => &[],
        };

        // Locate the dependencies in the project and add them to the stack
        for dependency in depends_on.iter() {
            if !added.contains(dependency) {
                let cmd = project
                    .command_opt(dependency)
                    .ok_or_else(|| anyhow::anyhow!("failed to find dependency {}", dependency))?
                    .clone();

                s1.push_back((cmd, Vec::new()));
                added.insert(dependency.clone());
            }
        }

        s2.push_back((command, additional_args))
    }

    Ok(s2)
}

pub async fn create_script(command: Command, args: Vec<String>) -> anyhow::Result<SequentialList> {
    // Construct the script from the command
    let command = match command {
        Command::Process(ProcessCmd {
            cmd: CmdArgs::Single(cmd),
            ..
        })
        | Command::Plain(cmd) => cmd,
        Command::Process(ProcessCmd {
            cmd: CmdArgs::Multiple(args),
            ..
        }) => quote_arguments(args),
        _ => {
            return Err(anyhow::anyhow!("No command given"));
        }
    };

    // Append the command line arguments
    let cli_args = quote_arguments(args);
    let full_script = format!("{command} {cli_args}");

    // Parse the shell command
    deno_task_shell::parser::parse(full_script.trim())
}
/// Executes the given command withing the specified project and with the given environment.
pub async fn execute_script(
    script: SequentialList,
    project: &Project,
    command_env: &HashMap<String, String>,
) -> anyhow::Result<i32> {
    // Execute the shell command
    Ok(deno_task_shell::execute(
        script,
        command_env.clone(),
        project.root(),
        Default::default(),
    )
    .await)
}

pub async fn execute_script_with_output(
    script: SequentialList,
    project: &Project,
    command_env: &HashMap<String, String>,
    input: Option<&[u8]>,
) -> RunOutput {
    let (stdin, mut stdin_writer) = pipe();
    if let Some(stdin) = input {
        stdin_writer.write_all(stdin).unwrap();
    }
    drop(stdin_writer); // prevent a deadlock by dropping the writer
    let (stdout, stdout_handle) = get_output_writer_and_handle();
    let (stderr, stderr_handle) = get_output_writer_and_handle();
    let state = ShellState::new(command_env.clone(), project.root(), Default::default());
    let code = execute_with_pipes(script, state, stdin, stdout, stderr).await;
    RunOutput {
        exit_code: code,
        stdout: stdout_handle.await.unwrap(),
        stderr: stderr_handle.await.unwrap(),
    }
}

fn quote_arguments(args: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    args.into_iter()
        // surround all the additional arguments in double quotes and santize any command
        // substitution
        .map(|a| format!("\"{}\"", a.as_ref().replace('"', "\\\"")))
        .join(" ")
}

/// CLI entry point for `pixi run`
pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    // Get the correctly ordered commands
    let mut ordered_commands = order_commands(args.command, &project)?;

    // Get the environment to run the commands in.
    let command_env = get_command_env(&project).await?;

    // Execute the commands in the correct order
    while let Some((command, args)) = ordered_commands.pop_back() {
        let script = create_script(command, args).await?;
        let status_code = execute_script(script, &project, &command_env).await?;
        if status_code != 0 {
            std::process::exit(status_code);
        }
    }

    Ok(())
}

/// Determine the environment variables to use when executing a command. This method runs the
/// activation scripts from the environment and stores the environment variables it added, it adds
/// environment variables set by the project and merges all of that with the system environment
/// variables.
pub async fn get_command_env(project: &Project) -> anyhow::Result<HashMap<String, String>> {
    // Get the prefix which we can then activate.
    let prefix = get_up_to_date_prefix(project).await?;

    // Get environment variables from the activation
    let activation_env = await_in_progress("activating environment", run_activation(prefix))
        .await
        .context("failed to activate environment")?;

    // Get environment variables from the manifest
    let manifest_env = get_metadata_env(project);

    // Construct command environment by concatenating the environments
    Ok(std::env::vars()
        .chain(activation_env.into_iter())
        .chain(manifest_env.into_iter())
        .collect())
}

/// Runs and caches the activation script.
async fn run_activation(prefix: Prefix) -> anyhow::Result<HashMap<String, String>> {
    let activator_result = tokio::task::spawn_blocking(move || {
        // Run and cache the activation script
        let shell: ShellEnum = ShellEnum::default();

        // Construct an activator for the script
        let activator = Activator::from_path(prefix.root(), shell, Platform::current())?;

        // Run the activation
        activator.run_activation(ActivationVariables {
            // Get the current PATH variable
            path: Default::default(),

            // Start from an empty prefix
            conda_prefix: None,

            // Prepending environment paths so they get found first.
            path_modification_behaviour: PathModificationBehaviour::Prepend,
        })
    })
    .await??;

    Ok(activator_result)
}
