use std::collections::{HashMap, HashSet};
use std::convert::identity;
use std::path::PathBuf;
use std::string::String;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use clap::Parser;
use dialoguer::theme::ColorfulTheme;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_config::{ConfigCli, ConfigCliActivation};
use pixi_manifest::TaskName;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::LocalSet;
use tracing::{error, info, Level};

use crate::{
    cli::cli_config::{LockFileUpdateConfig, PrefixUpdateConfig, WorkspaceConfig},
    environment::sanity_check_project,
    lock_file::UpdateLockFileOptions,
    task::{
        get_task_env, watcher::FileWatcher, AmbiguousTask, CanSkip, ExecutableTask,
        FailedToParseShellScript, InvalidWorkingDirectory, SearchEnvironments, TaskAndEnvironment,
        TaskGraph,
    },
    workspace::{errors::UnsupportedPlatformError, Environment},
    Workspace, WorkspaceLocator,
};

/// Runs task in the pixi environment and watch files for changes.
///
/// This command is used to run tasks in the pixi environment.
/// The tasks are killed and ran again when the files specified in `inputs` change.
/// It will activate the environment and run the task in the environment.
/// It is using the deno_task_shell to run the task.
///
/// `pixi watch` will also update the lockfile and install the environment if it is required.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, disable_help_flag = true)]
pub struct Args {
    /// The pixi task or a task shell command you want to run in the workspace's
    /// environment, which can be an executable in the environment's PATH.
    pub task: Vec<String>,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

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

/// CLI entry point for `pixi watch`
/// When running the sigints are ignored and child can react to them. As it
/// pleases.
pub async fn execute(args: Args) -> miette::Result<()> {
    let cli_config = args
        .activation_config
        .merge_config(args.config.clone().into());

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

    // Ensure that the lock-file is up-to-date.
    let lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_file_update_config.lock_file_usage(),
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
            ..UpdateLockFileOptions::default()
        })
        .await?;

    // Construct a task graph from the input arguments
    let search_environment = SearchEnvironments::from_opt_env(
        &workspace,
        explicit_environment.clone(),
        Some(best_platform),
    )
    .with_disambiguate_fn(disambiguate_task_interactive);

    let task_graph =
        TaskGraph::from_cmd_args(&workspace, &search_environment, args.task, args.skip_deps)?;

    // Currently only supporting a single task
    let topological_order = task_graph.topological_order();
    if topological_order.len() > 1 {
        eprintln!(
            "{}{}",
            console::Emoji("🚫 ", ""),
            console::style("Watch mode currently only supports single tasks without dependencies.")
                .yellow()
                .bold()
        );
        return Ok(());
    } else if topological_order.is_empty() {
        return Ok(());
    }

    // Get the single task
    let task_id = topological_order[0];
    let executable_task = ExecutableTask::from_task_graph(&task_graph, task_id);

    // If the task is not executable (e.g. an alias), we can't proceed
    if !executable_task.task().is_executable() {
        eprintln!(
            "{}{}",
            console::Emoji("🚫 ", ""),
            console::style("The specified task is not executable.")
                .yellow()
                .bold()
        );
        return Ok(());
    }

    tracing::info!("Task graph: {}", task_graph);

    // Create a broadcast channel for cancellation signals
    let (cancel_tx, _) = broadcast::channel::<()>(16);
    let cancel_tx = Arc::new(cancel_tx);

    // Set up Ctrl+C handler
    let ctrlc_should_exit_process = Arc::new(AtomicBool::new(true));
    let ctrlc_should_exit_process_clone = ctrlc_should_exit_process.clone();
    let cancel_tx_clone = cancel_tx.clone();

    ctrlc::set_handler(move || {
        reset_cursor();

        // Send cancellation signal
        let _ = cancel_tx_clone.send(());

        // Give tasks a moment to handle cancellation signal
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Exit the process if needed
        if ctrlc_should_exit_process_clone.load(Ordering::Relaxed) {
            exit_process_on_sigint();
        }
    })
    .into_diagnostic()?;

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

        // Display the task that would be executed
        if tracing::enabled!(Level::WARN) && !executable_task.task().is_custom() {
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

        return Ok(());
    }

    // Check task cache
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
            return Ok(());
        }
    };

    // If we don't have a command environment yet, we need to compute it
    let command_env = get_task_env(
        &executable_task.run_environment,
        args.clean_env || executable_task.task().clean_env(),
        Some(&lock_file.lock_file),
        workspace.config().force_activate(),
        workspace.config().experimental_activation_cache_usage(),
    )
    .await?;

    // Display the task that will be executed
    if tracing::enabled!(Level::WARN) && !executable_task.task().is_custom() {
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

    ctrlc_should_exit_process.store(false, Ordering::Relaxed);

    // Create a LocalSet for spawn_local
    let local = LocalSet::new();

    // Execute the task with file watching within the LocalSet
    let task_result = local
        .run_until(execute_task_with_watcher(
            &executable_task,
            &command_env,
            cancel_tx.clone(),
            ctrlc_should_exit_process.clone(),
        ))
        .await;

    match task_result {
        Ok(_) => {}
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

    #[error("shell error: {error}")]
    ShellError { error: String },

    #[error("watcher error: {0}")]
    WatcherError(String),
}

/// Execute a task with file watching using the notify-rs based watcher.
async fn execute_task_with_watcher(
    task: &ExecutableTask<'_>,
    command_env: &HashMap<String, String>,
    cancel_tx: Arc<broadcast::Sender<()>>,
    ctrlc_should_exit_process: Arc<AtomicBool>,
) -> Result<(), TaskExecutionError> {
    let mut cancel_rx = cancel_tx.subscribe();
    ctrlc_should_exit_process.store(false, Ordering::Relaxed);

    let Some(script) = task.as_deno_script()? else {
        return Err(TaskExecutionError::ShellError {
            error: "No script to execute".to_string(),
        });
    };
    let cwd = task.working_directory()?;

    // Get inputs directly from the task
    let inputs = if let Some(execute) = task.task().as_execute() {
        if let Some(inputs) = &execute.inputs {
            inputs
                .iter()
                .map(|i| task.project().root().join(i))
                .collect::<Vec<PathBuf>>()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let task_name = task.name().unwrap_or("unnamed").to_string();

    let was_cancelled = Arc::new(AtomicBool::new(false));
    let was_cancelled_clone = was_cancelled.clone();

    let script_clone = script.clone();
    let command_env_clone = command_env.clone();
    let cwd_clone = cwd.clone();

    let mut task_handle = Some(tokio::task::spawn_local(async move {
        let status_code = deno_task_shell::execute(
            script_clone,
            command_env_clone,
            &cwd_clone,
            Default::default(),
            Default::default(),
        )
        .await;

        if status_code != 0 && !was_cancelled_clone.load(Ordering::SeqCst) {
            tracing::error!("Task exited with status code: {}", status_code);
        }

        status_code
    }));

    // Configure file watcher with debouncing
    let debounce = Duration::from_millis(700);

    // Create the file watcher
    let mut watcher = FileWatcher::new(&cwd, &inputs, debounce)
        .await
        .map_err(|e| {
            error!("Failed to watch files: {}", e);
            TaskExecutionError::WatcherError(e.to_string())
        })?;

    loop {
        tokio::select! {
            // Handle Ctrl+C (SIGINT)
            _ = cancel_rx.recv() => {
                was_cancelled.store(true, Ordering::SeqCst);

                if task_handle.is_some() {
                    let handle = task_handle.take().expect("Task handle should be Some");
                    handle.abort();
                }

                eprintln!(
                    "{}{}",
                    console::Emoji("🛑 ", ""),
                    console::style(format!("Task {} was terminated", task_name))
                        .yellow()
                        .bold()
                );
                break;
            },

            event = watcher.next() => {
                match event {
                    Some(Ok(event)) => {
                        info!("Detected file change: {:?}", event.paths);

                        was_cancelled.store(true, Ordering::SeqCst);

                        if task_handle.is_some() {
                            let handle = task_handle.take().expect("Task handle should be Some");

                            // Abort the task handle to kill the thread
                            handle.abort();
                        }

                        let new_was_cancelled = Arc::new(AtomicBool::new(false));
                        let new_was_cancelled_clone = new_was_cancelled.clone();

                        eprintln!(
                            "{}{}{}{}",
                            console::Emoji("🔄 ", ""),
                            console::style("Reloading task: ").cyan().bold(),
                            console::style(task_name.clone()).green().bold(),
                            console::style(format!(" {}", task.display_command())).yellow()
                        );

                        was_cancelled.store(false, Ordering::SeqCst);

                        let script_clone = script.clone();
                        let command_env_clone = command_env.clone();
                        let cwd_clone = cwd.clone();

                        task_handle = Some(tokio::task::spawn_local(async move {
                            let status_code = deno_task_shell::execute(
                                script_clone,
                                command_env_clone,
                                &cwd_clone,
                                Default::default(),
                                Default::default(),
                            ).await;

                            if status_code != 0 && !new_was_cancelled_clone.load(Ordering::SeqCst) {
                                tracing::error!("Task exited with status code: {}", status_code);
                            }

                            status_code
                        }));

                    }
                    Some(Err(e)) => {
                        error!("Error watching files: {}", e);
                        break;
                    }
                    None => {
                        error!("File watcher closed unexpectedly");
                        break;
                    }
                }
            }
        }
    }

    ctrlc_should_exit_process.store(true, Ordering::Relaxed);

    // Check if the task is still running and get its result
    if let Some(handle) = task_handle {
        match handle.await {
            Ok(code) if code != 0 && !was_cancelled.load(Ordering::SeqCst) => {
                return Err(TaskExecutionError::NonZeroExitCode(code));
            }
            Err(e) => {
                tracing::error!("Error waiting for task: {}", e);
            }
            _ => {}
        }
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
