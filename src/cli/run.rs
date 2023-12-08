use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::string::String;

use clap::Parser;
use deno_task_shell::parser::SequentialList;
use deno_task_shell::{execute_with_pipes, pipe, ShellPipeWriter, ShellState};
use itertools::Itertools;
use miette::{miette, Context, Diagnostic, IntoDiagnostic};
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
use thiserror::Error;
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

#[derive(Debug, Error, Diagnostic)]
#[error("could not find the task '{task_name}'")]
pub struct MissingTaskError {
    task_name: String,
}

#[derive(Debug, Error, Diagnostic)]
#[error("deno task shell failed to parse '{script}': {error}")]
pub struct FailedToParseShellScript {
    script: String,
    error: String,
}

#[derive(Debug, Error, Diagnostic)]
#[error("invalid working directory '{path}'")]
pub struct InvalidWorkingDirectory {
    path: String,
}

/// A task that contains enough information to be able to execute it. The lifetime [`'p`] refers to
/// the lifetime of the project that contains the tasks.
#[derive(Debug, Clone)]
pub struct ExecutableTask<'p> {
    name: Option<String>,
    task: Cow<'p, Task>,
    additional_args: Vec<String>,
}

impl<'p> ExecutableTask<'p> {
    /// Parses command line arguments into an [`ExecutableTask`].
    pub fn from_cmd_args(
        project: &'p Project,
        args: Vec<String>,
        platform: Option<Platform>,
    ) -> Self {
        let mut args = args;

        if let Some(name) = args.first() {
            // Find the task in the project. First searches for platform specific tasks and falls
            // back to looking for the task in the default tasks.
            if let Some(task) = project.task_opt(name, platform) {
                return Self {
                    name: Some(args.remove(0)),
                    task: Cow::Borrowed(task),
                    additional_args: args,
                };
            }
        }

        // When no task is found, just execute the command verbatim.
        Self {
            name: None,
            task: Cow::Owned(
                Custom {
                    cmd: CmdArgs::from(args),
                    cwd: env::current_dir().ok(),
                }
                .into(),
            ),
            additional_args: vec![],
        }
    }

    /// Returns a list of [`ExecutableTask`]s that includes this task and its dependencies in the
    /// order they should be executed (topologically sorted).
    pub fn get_ordered_dependencies(
        self,
        project: &'p Project,
        platform: Option<Platform>,
    ) -> Result<Vec<Self>, MissingTaskError> {
        let mut sorted = Vec::new();
        visit(self, project, platform, &mut sorted, &mut HashSet::new())?;
        return Ok(sorted);

        fn visit<'p>(
            task: ExecutableTask<'p>,
            project: &'p Project,
            platform: Option<Platform>,
            sorted: &mut Vec<ExecutableTask<'p>>,
            visited: &mut HashSet<String>,
        ) -> Result<(), MissingTaskError> {
            // If the task has a name that we already visited we can immediately return.
            if let Some(name) = task.name.as_deref() {
                if visited.contains(name) {
                    return Ok(());
                }
                visited.insert(name.to_string());
            }

            // Locate the dependencies in the project and add them to the stack
            for dependency in task.task.depends_on() {
                let dependency =
                    project
                        .task_opt(dependency, platform)
                        .ok_or_else(|| MissingTaskError {
                            task_name: dependency.clone(),
                        })?;

                visit(
                    ExecutableTask {
                        name: Some(dependency.to_string()),
                        task: Cow::Borrowed(dependency),
                        additional_args: Vec::new(),
                    },
                    project,
                    platform,
                    sorted,
                    visited,
                )?;
            }

            sorted.push(task);
            Ok(())
        }
    }

    /// Returns a [`SequentialList`] which can be executed by deno task shell. Returns `None` if the
    /// command is not executable like in the case of an alias.
    pub fn as_deno_script(&self) -> Result<Option<SequentialList>, FailedToParseShellScript> {
        // Convert the task into an executable string
        let Some(task) = self.task.as_single_command() else {
            return Ok(None);
        };

        // Append the command line arguments
        let cli_args = quote_arguments(self.additional_args.iter().map(|arg| arg.as_str()));
        let full_script = format!("{task} {cli_args}");

        // Parse the shell command
        deno_task_shell::parser::parse(full_script.trim())
            .map_err(|e| FailedToParseShellScript {
                script: full_script,
                error: e.to_string(),
            })
            .map(Some)
    }

    /// Returns the working directory for this task.
    pub fn working_directory(
        &self,
        project: &'p Project,
    ) -> Result<PathBuf, InvalidWorkingDirectory> {
        Ok(match self.task.working_directory() {
            Some(cwd) if cwd.is_absolute() => cwd.to_path_buf(),
            Some(cwd) => {
                let abs_path = project.root().join(cwd);
                if !abs_path.exists() {
                    return Err(InvalidWorkingDirectory {
                        path: cwd.to_string_lossy().to_string(),
                    });
                }
                abs_path
            }
            None => project.root().to_path_buf(),
        })
    }
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
    let task_args = if args.task.len() == 1 {
        shlex::split(args.task[0].as_str())
            .ok_or(miette!("Could not split task, assuming non valid task"))?
    } else {
        args.task
    };
    tracing::debug!("Task parsed from run command: {:?}", task_args);

    // Get the correctly ordered commands
    let executable_tasks =
        ExecutableTask::from_cmd_args(&project, task_args, Some(Platform::current()))
            .get_ordered_dependencies(&project, Some(Platform::current()))?;

    // Get the environment to run the commands in.
    let command_env = get_task_env(&project, args.locked, args.frozen).await?;

    // Execute the commands in the correct order
    for task in executable_tasks {
        let Some(script) = task.as_deno_script()? else {
            continue;
        };
        let cwd = task.working_directory(&project)?;

        // Ignore CTRL+C
        // Specifically so that the child is responsible for its own signal handling
        // NOTE: one CTRL+C is registered it will always stay registered for the rest of the runtime of the program
        // which is fine when using run in isolation, however if we start to use run in conjunction with
        // some other command we might want to revaluate this.
        let ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

        // Showing which command is being run if the level and type allows it.
        if tracing::enabled!(Level::WARN) && !matches!(task.task.as_ref(), Task::Custom(_)) {
            eprintln!(
                "{}{} {}",
                console::style("✨ Pixi task: ").bold(),
                console::style(
                    &task
                        .task
                        .as_single_command()
                        .expect("The command should already be parsed")
                )
                .blue()
                .bold(),
                console::style(task.additional_args.join(" ")).blue(),
            );
        }

        let execute_future =
            deno_task_shell::execute(script, command_env.clone(), &cwd, Default::default());
        let status_code = tokio::select! {
            code = execute_future => code,
            // This should never exit
            _ = ctrl_c => { unreachable!("Ctrl+C should not be triggered") }
        };
        if status_code == 127 {
            let available_tasks = project
                .tasks(Some(Platform::current()))
                .into_keys()
                .sorted()
                .collect_vec();

            if !available_tasks.is_empty() {
                eprintln!(
                    "\nAvailable tasks:\n{}",
                    available_tasks.into_iter().format_with("\n", |name, f| {
                        f(&format_args!("\t{}", console::style(name).bold()))
                    })
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ordered_commands() {
        let file_content = r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        [tasks]
        root = "echo root"
        task1 = {cmd="echo task1", depends_on=["root"]}
        task2 = {cmd="echo task2", depends_on=["root"]}
        top = {cmd="echo top", depends_on=["task1","task2"]}
    "#;
        let project = Project::from_manifest_str(Path::new(""), file_content.to_string()).unwrap();

        let executable_tasks = ExecutableTask::from_cmd_args(
            &project,
            vec!["top".to_string(), "--test".to_string()],
            Some(Platform::current()),
        )
        .get_ordered_dependencies(&project, Some(Platform::current()))
        .unwrap();

        let ordered_task_names: Vec<_> = executable_tasks
            .iter()
            .map(|task| task.task.as_single_command().unwrap())
            .collect();

        assert_eq!(
            ordered_task_names,
            vec!["echo root", "echo task1", "echo task2", "echo top"]
        );

        // Also check if the arguments are passed correctly
        assert_eq!(
            executable_tasks.last().unwrap().additional_args,
            vec!["--test".to_string()]
        );
    }

    #[test]
    fn test_cycle_ordered_commands() {
        let file_content = r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        [tasks]
        root = {cmd="echo root", depends_on=["task1"]}
        task1 = {cmd="echo task1", depends_on=["root"]}
        task2 = {cmd="echo task2", depends_on=["root"]}
        top = {cmd="echo top", depends_on=["task1","task2"]}
    "#;
        let project = Project::from_manifest_str(Path::new(""), file_content.to_string()).unwrap();

        let executable_tasks = ExecutableTask::from_cmd_args(
            &project,
            vec!["top".to_string()],
            Some(Platform::current()),
        )
        .get_ordered_dependencies(&project, Some(Platform::current()))
        .unwrap();

        let ordered_task_names: Vec<_> = executable_tasks
            .iter()
            .map(|task| task.task.as_single_command().unwrap())
            .collect();

        assert_eq!(
            ordered_task_names,
            vec!["echo root", "echo task1", "echo task2", "echo top"]
        );
    }

    #[test]
    fn test_platform_ordered_commands() {
        let file_content = r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        [tasks]
        root = "echo root"
        task1 = {cmd="echo task1", depends_on=["root"]}
        task2 = {cmd="echo task2", depends_on=["root"]}
        top = {cmd="echo top", depends_on=["task1","task2"]}
        [target.linux-64.tasks]
        root = {cmd="echo linux", depends_on=["task1"]}
    "#;
        let project = Project::from_manifest_str(Path::new(""), file_content.to_string()).unwrap();

        let executable_tasks = ExecutableTask::from_cmd_args(
            &project,
            vec!["top".to_string()],
            Some(Platform::Linux64),
        )
        .get_ordered_dependencies(&project, Some(Platform::Linux64))
        .unwrap();

        let ordered_task_names: Vec<_> = executable_tasks
            .iter()
            .map(|task| task.task.as_single_command().unwrap())
            .collect();

        assert_eq!(
            ordered_task_names,
            vec!["echo linux", "echo task1", "echo task2", "echo top",]
        );
    }

    #[test]
    fn test_custom_command() {
        let file_content = r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
    "#;
        let project = Project::from_manifest_str(Path::new(""), file_content.to_string()).unwrap();

        let executable_tasks = ExecutableTask::from_cmd_args(
            &project,
            vec!["echo bla".to_string()],
            Some(Platform::Linux64),
        )
        .get_ordered_dependencies(&project, Some(Platform::Linux64))
        .unwrap();

        assert_eq!(executable_tasks.len(), 1);

        let task = executable_tasks.get(0).unwrap();
        assert!(matches!(task.task.as_ref(), &Task::Custom(_)));

        assert_eq!(task.task.as_single_command().unwrap(), r###""echo bla""###);
    }
}
