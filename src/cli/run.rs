use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    convert::identity,
    string::String,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use clap::Parser;
use dialoguer::theme::ColorfulTheme;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_config::ConfigCliActivation;
use pixi_manifest::TaskName;
use thiserror::Error;
use tracing::Level;

use crate::{
    cli::cli_config::{PrefixUpdateConfig, WorkspaceConfig},
    environment::sanity_check_project,
    lock_file::UpdateLockFileOptions,
    task::{
        get_task_env, AmbiguousTask, CanSkip, ExecutableTask, FailedToParseShellScript,
        InvalidWorkingDirectory, SearchEnvironments, TaskAndEnvironment, TaskGraph,
    },
    workspace::{
        errors::UnsupportedPlatformError,
        virtual_packages::verify_current_platform_has_required_virtual_packages, Environment,
    },
    Workspace, WorkspaceLocator,
};

/// Runs task in project.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, disable_help_flag = true)]
pub struct Args {
    /// The pixi task or a task shell command you want to run in the project's
    /// environment, which can be an executable in the environment's PATH.
    pub task: Vec<String>,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,

    #[clap(flatten)]
    pub activation_config: ConfigCliActivation,

    /// The environment to run the task in.
    #[arg(long, short)]
    pub environment: Option<String>,

    /// Use a clean environment to run the task
    ///
    /// Using this flag will ignore your current shell environment and use bare
    /// minimum environment to activate the pixi environment in.
    #[arg(long)]
    pub clean_env: bool,

    /// Don't run the dependencies of the task ('depends-on' field in the task
    /// definition)
    #[arg(long)]
    pub skip_deps: bool,

    /// Run the task in dry-run mode (only print the command that would run)
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    #[clap(long, action = clap::ArgAction::HelpLong)]
    pub help: Option<bool>,

    #[clap(short, action = clap::ArgAction::HelpShort)]
    pub h: Option<bool>,
}

/// CLI entry point for `pixi run`
/// When running the sigints are ignored and child can react to them. As it
/// pleases.
pub async fn execute(args: Args) -> miette::Result<()> {
    let cli_config = args
        .activation_config
        .merge_config(args.prefix_update_config.config.clone().into());

    // Load the workspace
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(cli_config);

    // Extract the passed in environment name.
    let environment = workspace.environment_from_name_or_env_var(args.environment.clone())?;

    // Find the environment to run the task in, if any were specified.
    let explicit_environment = if args.environment.is_none() && environment.is_default() {
        None
    } else {
        Some(environment.clone())
    };

    // Print all available tasks if no task is provided
    if args.task.is_empty() {
        command_not_found(&workspace, explicit_environment);
        return Ok(());
    }

    // Sanity check of prefix location
    sanity_check_project(&workspace).await?;

    let best_platform = environment.best_platform();

    // Verify that the current platform has the required virtual packages for the
    // environment.
    if let Some(ref explicit_environment) = explicit_environment {
        verify_current_platform_has_required_virtual_packages(explicit_environment)
            .into_diagnostic()?;
    }

    // Ensure that the lock-file is up-to-date.
    let mut lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.prefix_update_config.lock_file_usage(),
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
            ..UpdateLockFileOptions::default()
        })
        .await?;

    // dialoguer doesn't reset the cursor if it's aborted via e.g. SIGINT
    // So we do it ourselves.

    let ctrlc_should_exit_process = Arc::new(AtomicBool::new(true));
    let ctrlc_should_exit_process_clone = Arc::clone(&ctrlc_should_exit_process);

    ctrlc::set_handler(move || {
        reset_cursor();
        if ctrlc_should_exit_process_clone.load(Ordering::Relaxed) {
            exit_process_on_sigint();
        }
    })
    .into_diagnostic()?;

    // Construct a task graph from the input arguments
    let search_environment = SearchEnvironments::from_opt_env(
        &workspace,
        explicit_environment.clone(),
        Some(best_platform),
    )
    .with_disambiguate_fn(disambiguate_task_interactive);

    let task_graph =
        TaskGraph::from_cmd_args(&workspace, &search_environment, args.task, args.skip_deps)?;

    tracing::info!("Task graph: {}", task_graph);

    // Print dry-run message if dry-run mode is enabled
    if args.dry_run {
        eprintln!(
            "{}{}",
            console::Emoji("🌵 ", ""),
            console::style("Dry-run mode enabled - no tasks will be executed.")
                .yellow()
                .bold(),
        );
        eprintln!();
    }

    // Traverse the task graph in topological order and execute each individual
    // task.
    let mut task_idx = 0;
    let mut task_envs = HashMap::new();
    for task_id in task_graph.topological_order() {
        let executable_task = ExecutableTask::from_task_graph(&task_graph, task_id);

        // If the task is not executable (e.g. an alias), we skip it. This ensures we
        // don't instantiate a prefix for an alias.
        if !executable_task.task().is_executable() {
            continue;
        }

        // Showing which command is being run if the level and type allows it.
        if tracing::enabled!(Level::WARN) && !executable_task.task().is_custom() {
            if task_idx > 0 {
                // Add a newline between task outputs
                eprintln!();
            }
            eprintln!(
                "{}{}{}{}{}{}{}",
                console::Emoji("✨ ", ""),
                console::style("Pixi task (").bold(),
                console::style(executable_task.name().unwrap_or("unnamed"))
                    .green()
                    .bold(),
                // Only print environment if multiple environments are available
                if workspace.environments().len() > 1 {
                    format!(
                        " in {}",
                        executable_task.run_environment.name().fancy_display()
                    )
                } else {
                    "".to_string()
                },
                console::style("): ").bold(),
                executable_task.display_command(),
                if let Some(description) = executable_task.task().description() {
                    console::style(format!(": ({})", description)).yellow()
                } else {
                    console::style("".to_string()).yellow()
                }
            );
        }

        // on dry-run mode, we just print the command and skip the execution
        if args.dry_run {
            task_idx += 1;
            continue;
        }

        // check task cache
        let task_cache = match executable_task
            .can_skip(&lock_file.lock_file)
            .await
            .into_diagnostic()?
        {
            CanSkip::No(cache) => cache,
            CanSkip::Yes => {
                eprintln!(
                    "Task '{}' can be skipped (cache hit) 🚀",
                    console::style(executable_task.name().unwrap_or("")).bold()
                );
                task_idx += 1;
                continue;
            }
        };

        // If we don't have a command environment yet, we need to compute it. We lazily
        // compute the task environment because we only need the environment if
        // a task is actually executed.
        let task_env: &_ = match task_envs.entry(executable_task.run_environment.clone()) {
            Entry::Occupied(env) => env.into_mut(),
            Entry::Vacant(entry) => {
                // Ensure there is a valid prefix
                lock_file
                    .prefix(
                        &executable_task.run_environment,
                        args.prefix_update_config.update_mode(),
                    )
                    .await?;

                let command_env = get_task_env(
                    &executable_task.run_environment,
                    args.clean_env || executable_task.task().clean_env(),
                    Some(&lock_file.lock_file),
                    workspace.config().force_activate(),
                    workspace.config().experimental_activation_cache_usage(),
                )
                .await?;
                entry.insert(command_env)
            }
        };

        ctrlc_should_exit_process.store(false, Ordering::Relaxed);

        // Execute the task itself within the command environment. If one of the tasks
        // failed with a non-zero exit code, we exit this parent process with
        // the same code.
        match execute_task(&executable_task, task_env).await {
            Ok(_) => {
                task_idx += 1;
            }
            Err(TaskExecutionError::NonZeroExitCode(code)) => {
                if code == 127 {
                    command_not_found(&workspace, explicit_environment);
                }
                std::process::exit(code);
            }
            Err(err) => return Err(err.into()),
        }

        // Handle CTRL-C ourselves again
        ctrlc_should_exit_process.store(true, Ordering::Relaxed);

        // Update the task cache with the new hash
        executable_task
            .save_cache(&lock_file, task_cache)
            .await
            .into_diagnostic()?;
    }

    Ok(())
}

/// Called when a command was not found.
fn command_not_found<'p>(workspace: &'p Workspace, explicit_environment: Option<Environment<'p>>) {
    let available_tasks: HashSet<TaskName> =
        if let Some(explicit_environment) = explicit_environment {
            explicit_environment.get_filtered_tasks()
        } else {
            workspace
                .environments()
                .into_iter()
                .filter(|env| verify_current_platform_has_required_virtual_packages(env).is_ok())
                .flat_map(|env| env.get_filtered_tasks())
                .collect()
        };

    if !available_tasks.is_empty() {
        eprintln!(
            "\nAvailable tasks:\n{}",
            available_tasks
                .into_iter()
                .sorted()
                .format_with("\n", |name, f| {
                    f(&format_args!("\t{}", name.fancy_display().bold()))
                })
        );
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
    UnsupportedPlatformError(#[from] UnsupportedPlatformError),
}

/// Called to execute a single command.
///
/// This function is called from [`execute`].
async fn execute_task(
    task: &ExecutableTask<'_>,
    command_env: &HashMap<String, String>,
) -> Result<(), TaskExecutionError> {
    let Some(script) = task.as_deno_script()? else {
        return Ok(());
    };
    let cwd = task.working_directory()?;

    let status_code = deno_task_shell::execute(
        script,
        command_env.clone(),
        &cwd,
        Default::default(),
        Default::default(),
    )
    .await;

    if status_code != 0 {
        return Err(TaskExecutionError::NonZeroExitCode(status_code));
    }

    Ok(())
}

/// Called to disambiguate between environments to run a task in.
fn disambiguate_task_interactive<'p>(
    problem: &AmbiguousTask<'p>,
) -> Option<TaskAndEnvironment<'p>> {
    let environment_names = problem
        .environments
        .iter()
        .map(|(env, _)| env.name())
        .collect_vec();
    let theme = ColorfulTheme {
        active_item_style: console::Style::new().for_stderr().magenta(),
        ..ColorfulTheme::default()
    };

    dialoguer::Select::with_theme(&theme)
        .with_prompt(format!(
            "The task '{}' {}can be run in multiple environments.\n\nPlease select an environment to run the task in:",
            problem.task_name.fancy_display(),
            if let Some(dependency) = &problem.depended_on_by {
                format!("(depended on by '{}') ", dependency.0.fancy_display())
            } else {
                String::new()
            }
        ))
        .report(false)
        .items(&environment_names)
        .default(0)
        .interact_opt()
        .map_or(None, identity)
        .map(|idx| problem.environments[idx].clone())
}

/// `dialoguer` doesn't clean up your term if it's aborted via e.g. `SIGINT` or
/// other exceptions: https://github.com/console-rs/dialoguer/issues/188.
///
/// `dialoguer`, as a library, doesn't want to mess with signal handlers,
/// but we, as an application, are free to mess with signal handlers if we feel
/// like it, since we own the process.
/// This function was taken from https://github.com/dnjstrom/git-select-branch/blob/16c454624354040bc32d7943b9cb2e715a5dab92/src/main.rs#L119
fn reset_cursor() {
    let term = console::Term::stdout();
    let _ = term.show_cursor();
}

/// Exit the process with the appropriate exit code for a SIGINT.
fn exit_process_on_sigint() {
    // https://learn.microsoft.com/en-us/cpp/c-runtime-library/signal-constants
    #[cfg(target_os = "windows")]
    std::process::exit(3);

    // POSIX compliant OSs: 128 + SIGINT (2)
    #[cfg(not(target_os = "windows"))]
    std::process::exit(130);
}
