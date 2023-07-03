use anyhow::Context;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::path::PathBuf;
use std::string::String;

use clap::Parser;
use is_executable::IsExecutable;
use itertools::Itertools;
use rattler_conda_types::Platform;

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

pub async fn order_commands(
    commands: Vec<String>,
    project: &Project,
) -> anyhow::Result<VecDeque<Command>> {
    let command: Vec<_> = commands.iter().map(|c| c.to_string()).collect();

    let (command_name, command) = command
        .first()
        .and_then(|cmd_name| {
            project
                .command_opt(cmd_name)
                .map(|cmd| (Some(cmd_name.clone()), cmd.clone()))
        })
        .unwrap_or_else(|| {
            (
                None,
                Command::Process(ProcessCmd {
                    cmd: CmdArgs::Multiple(commands),
                    depends_on: vec![],
                }),
            )
        });

    // Perform post order traversal of the commands and their `depends_on` to make sure they are
    // executed in the right order.
    let mut s1 = VecDeque::new();
    let mut s2 = VecDeque::new();
    let mut added = HashSet::new();

    // Add the command specified on the command line first
    s1.push_back(command);
    if let Some(command_name) = command_name {
        added.insert(command_name);
    }

    while let Some(command) = s1.pop_back() {
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

                s1.push_back(cmd);
                added.insert(dependency.clone());
            }
        }

        s2.push_back(command)
    }

    Ok(s2)
}

pub async fn create_command(
    command: Command,
    project: &Project,
    command_env: &HashMap<String, String>,
) -> anyhow::Result<Option<std::process::Command>> {
    // Command arguments
    let args = match command {
        Command::Process(ProcessCmd {
            cmd: CmdArgs::Single(cmd),
            ..
        })
        | Command::Plain(cmd) => {
            shlex::split(&cmd).ok_or_else(|| anyhow::anyhow!("invalid quoted command arguments"))?
        }
        Command::Process(ProcessCmd {
            cmd: CmdArgs::Multiple(cmd),
            ..
        }) => cmd,
        _ => {
            // Nothing to do
            return Ok(None);
        }
    };

    // Format the arguments
    let (cmd, formatted_args) = format_execute_command(
        project,
        &command_env
            .get("PATH")
            .or_else(|| command_env.get("Path"))
            .into_iter()
            .flat_map(std::env::split_paths)
            .collect_vec(),
        &args,
    )?;

    // Construct a command to execute
    let mut cmd = std::process::Command::new(cmd);
    cmd.args(formatted_args);
    cmd.envs(command_env);

    Ok(Some(cmd))
}

/// CLI entry point for `pixi run`
pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    // Get the correctly ordered commands
    let mut ordered_commands = order_commands(args.command, &project).await?;

    // Get environment variables from the activation
    let activation_env = await_in_progress("activating environment", run_activation(&project))
        .await
        .context("failed to activate environment")?;

    // Get environment variables from the manifest
    let manifest_env = get_metadata_env(&project);

    // Construct command environment
    let command_env = activation_env
        .into_iter()
        .chain(manifest_env.into_iter())
        .collect();

    // Execute the commands in the correct order
    while let Some(command) = ordered_commands.pop_back() {
        if let Some(mut command) = create_command(command, &project, &command_env).await? {
            let status = command.spawn()?.wait()?.code().unwrap_or(1);
            if status != 0 {
                std::process::exit(status);
            }
        }
    }

    Ok(())
}

/// Runs and caches the activation script.
async fn run_activation(project: &Project) -> anyhow::Result<HashMap<String, String>> {
    // Construct an activator so we can run commands from the environment
    let prefix = get_up_to_date_prefix(project).await?;

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

/// Given a command and arguments to invoke it, format it so that it is as generalized as possible.
///
/// The executable is also canonicalized. This means the executable path is looked up. If the
/// executable is not found either in the environment or in the project root an error is returned.
fn format_execute_command(
    project: &Project,
    path: &[PathBuf],
    args: &[String],
) -> anyhow::Result<(PathBuf, Vec<String>)> {
    // Determine the command location
    let command = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let command_path = find_command(command, project.root(), path.iter().map(|p| p.as_path()))
        .ok_or_else(|| anyhow::anyhow!("could not find executable '{command}'"))?;

    // Format all the commands and quote them properly.
    Ok((
        command_path,
        args.iter()
            .skip(1)
            .map(|arg| shlex::quote(arg).into_owned())
            .collect(),
    ))
}

// Locate the specified command name in the project or environment
fn find_command<'a>(
    executable_name: &str,
    project_root: &'a Path,
    prefix_paths: impl IntoIterator<Item = &'a Path>,
) -> Option<PathBuf> {
    let executable_path = Path::new(executable_name);

    // Iterate over all search paths
    for search_path in [project_root].into_iter().chain(prefix_paths) {
        let absolute_executable_path = search_path.join(executable_path);

        // Try to locate an executable at this location
        if let Some(executable_path) = find_canonical_executable_path(&absolute_executable_path) {
            return Some(executable_path);
        }
    }

    None
}

// Given a relative executable path, try to find the canonical path
fn find_canonical_executable_path(path: &Path) -> Option<PathBuf> {
    // If the path already points to an existing executable there is nothing to do.
    match dunce::canonicalize(path) {
        Ok(path) if path.is_executable() => return Some(path),
        _ => {}
    }

    // Get executable extensions and see if by adding the extension we can turn it into a valid
    // path.
    for ext in executable_extensions() {
        let with_ext = path.with_extension(ext);
        match dunce::canonicalize(with_ext) {
            Ok(path) if path.is_executable() => return Some(path),
            _ => {}
        }
    }

    None
}

/// Returns all file extensions that are considered for executable files.
#[cfg(windows)]
fn executable_extensions() -> &'static [String] {
    use once_cell::sync::Lazy;
    static PATHEXT: Lazy<Vec<String>> = Lazy::new(|| {
        if let Some(pathext) = std::env::var_os("PATHEXT") {
            pathext
                .to_string_lossy()
                .split(';')
                // Filter out empty tokens and ';' at the end
                .filter(|f| f.len() > 1)
                // Cut off the leading '.' character
                .map(|ext| ext[1..].to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    });
    PATHEXT.as_slice()
}

/// Returns all file extensions that are considered for executable files.
#[cfg(not(windows))]
fn executable_extensions() -> &'static [String] {
    &[]
}
