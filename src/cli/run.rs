use std::{collections::HashMap, path::PathBuf, string::String};

use clap::Parser;
use itertools::Itertools;
use miette::{miette, Diagnostic};
use rattler_conda_types::Platform;

use crate::activation::get_activation_env;
use crate::project::errors::UnsupportedPlatformError;
use crate::task::{
    ExecutableTask, FailedToParseShellScript, InvalidWorkingDirectory, TraversalError,
};
use crate::Project;

use crate::project::manifest::EnvironmentName;
use thiserror::Error;
use tracing::Level;

/// Runs task in project.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// The task you want to run in the projects environment.
    pub task: Vec<String>,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    #[arg(long, short)]
    pub environment: Option<String>,
}

/// CLI entry point for `pixi run`
/// When running the sigints are ignored and child can react to them. As it pleases.
pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let environment_name = args
        .environment
        .map_or_else(|| EnvironmentName::Default, EnvironmentName::Named);
    let environment = project
        .environment(&environment_name)
        .ok_or_else(|| miette::miette!("unknown environment '{environment_name}'"))?;

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

    // Get the task to execute
    // TODO: Make this environment specific
    let executable_task =
        ExecutableTask::from_cmd_args(&project, task_args, Some(Platform::current()));

    // Get the environment to run the commands in.
    let command_env = get_activation_env(&environment, args.lock_file_usage.into()).await?;

    // Traverse the task and its dependencies. Execute each task in order.
    match executable_task
        .traverse(
            (),
            |_, task| execute_task(task, &command_env),
            |_, _task| async { true },
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(TaskExecutionError::NonZeroExitCode(code)) => {
            // If one of the tasks failed with a non-zero exit code, we exit this parent process
            // with the same code.
            std::process::exit(code);
        }
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Error, Diagnostic)]
enum TaskExecutionError {
    #[error("the script exited with a non-zero exit code {0}")]
    NonZeroExitCode(i32),

    #[error(transparent)]
    FailedToParseShellScript(#[from] FailedToParseShellScript),

    #[error(transparent)]
    InvalidWorkingDirectory(#[from] InvalidWorkingDirectory),

    #[error(transparent)]
    TraverseError(#[from] TraversalError),

    #[error(transparent)]
    UnsupportedPlatformError(#[from] UnsupportedPlatformError),
}

/// Called to execute a single command.
///
/// This function is called from [`execute`].
async fn execute_task<'p>(
    task: ExecutableTask<'p>,
    command_env: &HashMap<String, String>,
) -> Result<(), TaskExecutionError> {
    let Some(script) = task.as_deno_script()? else {
        return Ok(());
    };
    let cwd = task.working_directory()?;

    // Ignore CTRL+C
    // Specifically so that the child is responsible for its own signal handling
    // NOTE: one CTRL+C is registered it will always stay registered for the rest of the runtime of the program
    // which is fine when using run in isolation, however if we start to use run in conjunction with
    // some other command we might want to revaluate this.
    let ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    // Showing which command is being run if the level and type allows it.
    if tracing::enabled!(Level::WARN) && !task.task().is_custom() {
        eprintln!(
            "{}{}",
            console::style("✨ Pixi task: ").bold(),
            task.display_command(),
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
        let available_tasks = task
            .project()
            .default_environment()
            .tasks(Some(Platform::current()))?
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
        return Err(TaskExecutionError::NonZeroExitCode(status_code));
    }

    Ok(())
}
