use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::path::{Path, PathBuf};
use std::string::String;

use clap::Parser;
use deno_task_shell::parser::SequentialList;
use deno_task_shell::{execute_with_pipes, pipe, ShellPipeWriter, ShellState};
use itertools::Itertools;
use miette::{miette, Context, IntoDiagnostic};
use rattler_conda_types::Platform;

use crate::prefix::Prefix;
use crate::progress::await_in_progress;
use crate::project::environment::get_metadata_env;
use crate::task::{quote_arguments, CmdArgs, Custom, Task};
use crate::{environment::get_up_to_date_prefix, Project};
use rattler_shell::{
    activation::{ActivationVariables, Activator, PathModificationBehavior},
    shell::ShellEnum,
};
use tokio::task::JoinHandle;
use tracing::Level;

/// Runs task in project.
#[derive(Default, Debug)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs task in project.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// The task you want to run in the projects environment.
    pub task: Vec<String>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Require pixi.lock is up-to-date
    #[clap(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Don't check if pixi.lock is up-to-date, install as lockfile states
    #[clap(long, conflicts_with = "locked")]
    pub frozen: bool,
}

pub fn order_tasks(
    tasks: Vec<String>,
    project: &Project,
) -> miette::Result<VecDeque<(Task, Vec<String>)>> {
    let tasks: Vec<_> = tasks.iter().map(|c| c.to_string()).collect();

    // Find the command in the tasks.
    let (task_name, task, additional_args) = tasks
        .first()
        // First search in the target specific tasks
        .and_then(|cmd_name| {
            project
                .target_specific_tasks(Platform::current())
                .get(cmd_name.as_str())
                .map(|&cmd| {
                    (
                        Some(cmd_name.clone()),
                        cmd.clone(),
                        tasks[1..].iter().cloned().collect_vec(),
                    )
                })
        })
        // If it isn't found in the target specific tasks try to find it in the default tasks.
        .or_else(|| {
            tasks.first().and_then(|cmd_name| {
                project.task_opt(cmd_name).map(|cmd| {
                    (
                        Some(cmd_name.clone()),
                        cmd.clone(),
                        tasks[1..].iter().cloned().collect_vec(),
                    )
                })
            })
        })
        // When no task is found, just execute the command.
        .unwrap_or_else(|| {
            (
                None,
                Custom {
                    cmd: CmdArgs::from(tasks),
                    cwd: Some(env::current_dir().unwrap_or(project.root().to_path_buf())),
                }
                .into(),
                Vec::new(),
            )
        });

    // Perform post order traversal of the tasks and their `depends_on` to make sure they are
    // executed in the right order.
    let mut s1 = VecDeque::new();
    let mut s2 = VecDeque::new();
    let mut added = HashSet::new();

    // Add the command specified on the command line first
    s1.push_back((task, additional_args));
    if let Some(task_name) = task_name {
        added.insert(task_name);
    }

    while let Some((task, additional_args)) = s1.pop_back() {
        // Get the dependencies of the command
        let depends_on = task.depends_on();

        // Locate the dependencies in the project and add them to the stack
        for dependency in depends_on.iter() {
            if !added.contains(dependency) {
                let cmd = project
                    .target_specific_tasks(Platform::current())
                    .get(dependency.as_str())
                    .copied()
                    // If there is no target specific task try to find it in the default tasks.
                    .or_else(|| project.task_opt(dependency))
                    .ok_or_else(|| miette::miette!("failed to find dependency {}", dependency))?;

                s1.push_back((cmd.clone(), Vec::new()));
                added.insert(dependency.clone());
            }
        }

        if task.is_executable() {
            s2.push_back((task, additional_args))
        }
    }

    Ok(s2)
}

pub async fn create_script(task: &Task, args: Vec<String>) -> miette::Result<SequentialList> {
    // Construct the script from the task
    let task = task
        .as_single_command()
        .ok_or_else(|| miette::miette!("the task does not provide a runnable command"))?;

    // Append the command line arguments
    let cli_args = quote_arguments(args.iter().map(|arg| arg.as_str()));
    let full_script = format!("{task} {cli_args}");

    // Parse the shell command
    deno_task_shell::parser::parse(full_script.trim()).map_err(|e| miette!("{e}"))
}

/// Select a working directory based on a given path or the project.
pub fn select_cwd(path: Option<&Path>, project: &Project) -> miette::Result<PathBuf> {
    Ok(match path {
        Some(cwd) if cwd.is_absolute() => cwd.to_path_buf(),
        Some(cwd) => {
            let abs_path = project.root().join(cwd);
            if !abs_path.exists() {
                miette::bail!("Can't find the 'cwd': '{}'", abs_path.display());
            }
            abs_path
        }
        None => project.root().to_path_buf(),
    })
}
/// Executes the given command within the specified project and with the given environment.
pub async fn execute_script(
    script: SequentialList,
    command_env: &HashMap<String, String>,
    cwd: &Path,
) -> miette::Result<i32> {
    // Execute the shell command
    Ok(deno_task_shell::execute(script, command_env.clone(), cwd, Default::default()).await)
}

pub async fn execute_script_with_output(
    script: SequentialList,
    cwd: &Path,
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
    let state = ShellState::new(command_env.clone(), cwd, Default::default());
    let code = execute_with_pipes(script, state, stdin, stdout, stderr).await;
    RunOutput {
        exit_code: code,
        stdout: stdout_handle.await.unwrap(),
        stderr: stderr_handle.await.unwrap(),
    }
}

/// CLI entry point for `pixi run`
/// When running the sigints are ignored and child can react to them. As it pleases.
pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    // Split 'task' into arguments if it's a single string, supporting commands like:
    // `"test 1 == 0 || echo failed"` or `"echo foo && echo bar"` or `"echo 'Hello World'"`
    // This prevents shell interpretation of pixi run inputs.
    // Use as-is if 'task' already contains multiple elements.
    let task = if args.task.len() == 1 {
        shlex::split(args.task[0].as_str())
            .ok_or(miette!("Could not split task, assuming non valid task"))?
    } else {
        args.task
    };
    tracing::debug!("Task parsed from run command: {:?}", task);

    // Get the correctly ordered commands
    let mut ordered_commands = order_tasks(task, &project)?;

    // Get the environment to run the commands in.
    let command_env = get_task_env(&project, args.locked, args.frozen).await?;

    // Execute the commands in the correct order
    while let Some((command, arguments)) = ordered_commands.pop_back() {
        let cwd = select_cwd(command.working_directory(), &project)?;
        // Ignore CTRL+C
        // Specifically so that the child is responsible for its own signal handling
        // NOTE: one CTRL+C is registered it will always stay registered for the rest of the runtime of the program
        // which is fine when using run in isolation, however if we start to use run in conjunction with
        // some other command we might want to revaluate this.
        let ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });
        let script = create_script(&command, arguments).await?;

        // Showing which command is being run if the level and type allows it.
        if tracing::enabled!(Level::WARN) && !matches!(command, Task::Custom(_)) {
            eprintln!(
                "{}{}",
                console::style("✨ Pixi task: ").bold(),
                console::style(
                    &command
                        .as_single_command()
                        .expect("The command should already be parsed")
                )
                .blue()
                .bold()
            );
        }

        let status_code = tokio::select! {
            code = execute_script(script, &command_env, &cwd) => code?,
            // This should never exit
            _ = ctrl_c => { unreachable!("Ctrl+C should not be triggered") }
        };
        if status_code == 127 {
            let formatted: String = project
                .tasks(Some(Platform::current()))
                .into_keys()
                .sorted()
                .map(|name| format!("\t{}\n", console::style(name).bold()))
                .collect();

            eprintln!("\nAvailable tasks:\n{}", formatted);
        }
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
pub async fn get_task_env(
    project: &Project,
    frozen: bool,
    locked: bool,
) -> miette::Result<HashMap<String, String>> {
    // Get the prefix which we can then activate.
    let prefix = get_up_to_date_prefix(project, frozen, locked).await?;

    // Get environment variables from the activation
    let activation_env = run_activation_async(project, prefix).await?;

    // Get environment variables from the manifest
    let manifest_env = get_metadata_env(project);

    // Construct command environment by concatenating the environments
    Ok(std::env::vars()
        .chain(activation_env.into_iter())
        .chain(manifest_env.into_iter())
        .collect())
}

/// Runs the activation script asynchronously. This function also adds a progress bar.
pub async fn run_activation_async(
    project: &Project,
    prefix: Prefix,
) -> miette::Result<HashMap<String, String>> {
    let additional_activation_scripts = project.activation_scripts(Platform::current())?;
    await_in_progress(
        "activating environment",
        run_activation(prefix, additional_activation_scripts.into_iter().collect()),
    )
    .await
    .wrap_err("failed to activate environment")
}

/// Runs and caches the activation script.
async fn run_activation(
    prefix: Prefix,
    additional_activation_scripts: Vec<PathBuf>,
) -> miette::Result<HashMap<String, String>> {
    let activator_result = tokio::task::spawn_blocking(move || {
        // Run and cache the activation script
        let shell: ShellEnum = ShellEnum::default();

        // Construct an activator for the script
        let mut activator = Activator::from_path(prefix.root(), shell, Platform::current())?;
        activator
            .activation_scripts
            .extend(additional_activation_scripts);

        // Run the activation
        activator.run_activation(ActivationVariables {
            // Get the current PATH variable
            path: Default::default(),

            // Start from an empty prefix
            conda_prefix: None,

            // Prepending environment paths so they get found first.
            path_modification_behaviour: PathModificationBehavior::Prepend,
        })
    })
    .await
    .into_diagnostic()?
    .into_diagnostic()?;

    Ok(activator_result)
}

/// Helper function to create a pipe that we can get the output from.
fn get_output_writer_and_handle() -> (ShellPipeWriter, JoinHandle<String>) {
    let (reader, writer) = pipe();
    let handle = reader.pipe_to_string_handle();
    (writer, handle)
}
