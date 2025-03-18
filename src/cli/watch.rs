use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::convert::identity;
use std::process::Stdio;
use std::string::String;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use clap::Parser;
use dashmap::DashMap;
use dialoguer::theme::ColorfulTheme;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_config::ConfigCliActivation;
use pixi_manifest::TaskName;
use thiserror::Error;
use tokio::{process::Command, sync::broadcast};
use tracing::Level;

use crate::{
    cli::cli_config::{PrefixUpdateConfig, WorkspaceConfig},
    environment::sanity_check_project,
    lock_file::UpdateLockFileOptions,
    task::{
        get_task_env, AmbiguousTask, CanSkip, ExecutableTask, FailedToParseShellScript,
        FileWatcher, InvalidWorkingDirectory, SearchEnvironments, TaskAndEnvironment, TaskGraph,
    },
    workspace::{errors::UnsupportedPlatformError, Environment},
    Workspace, WorkspaceLocator,
};

/// Runs task in project.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, disable_help_flag = true)]
pub struct Args {
    /// The pixi task or a task shell command you want to run with watcher in the project's
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

/// CLI entry point for `pixi watch`
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

    // Ensure that the lock-file is up-to-date.
    let mut lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.prefix_update_config.lock_file_usage(),
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

    tracing::info!("Task graph: {}", task_graph);

    // Create a broadcast channel for cancellation signals
    let (cancel_tx, _) = broadcast::channel::<()>(16);
    let cancel_tx = Arc::new(cancel_tx);

    // Track running tasks in reverse topological order
    let running_tasks: Arc<DashMap<String, Option<tokio::process::Child>>> =
        Arc::new(DashMap::new());
    let running_tasks_for_handler = running_tasks.clone();

    // Set up Ctrl+C handler
    let ctrlc_should_exit_process = Arc::new(AtomicBool::new(true));
    let ctrlc_should_exit_process_clone = ctrlc_should_exit_process.clone();
    let cancel_tx_clone = cancel_tx.clone();

    ctrlc::set_handler(move || {
        reset_cursor();

        // Send cancellation signal to all running tasks regardless of exit state
        let _ = cancel_tx_clone.send(());

        // Get task names from running tasks
        let task_names: Vec<String> = running_tasks_for_handler
            .iter()
            .filter_map(|entry| {
                if entry.value().is_some() {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        if !task_names.is_empty() {
            // Print what tasks are being canceled
            for task_name in &task_names {
                eprintln!(
                    "{}{}",
                    console::Emoji("🛑 ", ""),
                    console::style(format!("Cancelling task: {}", task_name))
                        .yellow()
                        .bold()
                );
            }

            // Give tasks a moment to handle cancellation signal
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

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
    }

    // Traverse the task graph in topological order and execute each individual
    // task.
    let mut task_idx = 0;
    let mut task_envs = HashMap::new();
    let topological_order = task_graph.topological_order();

    for (order_idx, task_id) in topological_order.iter().enumerate() {
        let executable_task = ExecutableTask::from_task_graph(&task_graph, *task_id);

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
        // failed with a non-zero exit code, we exit this parent process with the same code.
        match execute_task_with_watched_files(
            &executable_task,
            task_env,
            cancel_tx.clone(),
            ctrlc_should_exit_process.clone(),
            running_tasks.clone(),
            format!("{:?}:{}", task_id, order_idx),
            order_idx,
        )
        .await
        {
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
}

/// Kill a running task gracefully
async fn kill_task(
    child: &mut tokio::process::Child,
    task_id: &str,
) -> Result<(), TaskExecutionError> {
    tracing::info!("Terminating task: {}", task_id);

    println!("Terminating task: {}", task_id);

    // Try to kill the process gracefully
    match child.kill().await {
        Ok(_) => {
            eprintln!(
                "{}{}",
                console::Emoji("🛑 ", ""),
                console::style(format!("Task {} was terminated", task_id))
                    .yellow()
                    .bold()
            );
            Ok(())
        }
        Err(e) => {
            // Process might have already exited
            if e.kind() == std::io::ErrorKind::InvalidInput {
                // This is fine - process already exited
                Ok(())
            } else {
                Err(TaskExecutionError::ShellError {
                    error: format!("Failed to kill task: {}", e),
                })
            }
        }
    }
}

/// Reload a task by killing the current one and starting a new one
async fn reload_task(
    task: &ExecutableTask<'_>,
    command_env: &HashMap<String, String>,
    current_child: &mut Option<tokio::process::Child>,
    cancel_rx: &mut broadcast::Receiver<()>,
) -> Result<(), TaskExecutionError> {
    let task_name = task.name().unwrap_or("unnamed");

    // First kill the current task if it exists
    if let Some(child) = current_child.as_mut() {
        println!("Killing task kir to soroush: {}", task_name);
        let _ = kill_task(child, task_name).await;
        *current_child = None;
    }

    // Print reloading message
    eprintln!(
        "{}{}{} {}",
        console::Emoji("🔄 ", ""),
        console::style("Reloading task: ").cyan().bold(),
        console::style(task_name).green().bold(),
        console::style(task.display_command().to_string())
            .yellow()
            .bold()
    );

    // Execute the task
    match execute_task(task, command_env, cancel_rx).await {
        Ok(child) => {
            *current_child = Some(child);
            Ok(())
        }
        Err(e) => {
            tracing::error!("Error reloading task {}: {}", task_name, e);
            Err(e)
        }
    }
}

/// Function to execute a single task.
async fn execute_task(
    task: &ExecutableTask<'_>,
    command_env: &HashMap<String, String>,
    _cancel_rx: &mut broadcast::Receiver<()>,
) -> Result<tokio::process::Child, TaskExecutionError> {
    let task_name = task.name().unwrap_or("unnamed");
    tracing::info!("Executing task: {}", task_name);

    let Some(_script) = task.as_deno_script()? else {
        return Err(TaskExecutionError::ShellError {
            error: "No script to execute".to_string(),
        });
    };
    let cwd = task.working_directory()?;
    let command_env = command_env.clone();

    // We can't use deno_task_shell directly across threads, so instead:
    // 1. We'll extract the command to run using the shell
    let command_to_run = match task.full_command() {
        Some(cmd) => cmd,
        None => {
            return Err(TaskExecutionError::ShellError {
                error: "No command to execute".to_string(),
            })
        }
    };

    let child = Command::new("sh")
        .arg("-c")
        .arg(&command_to_run)
        .current_dir(&cwd)
        .env_clear()
        .envs(command_env)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit())
        .spawn()
        .map_err(|e| TaskExecutionError::ShellError {
            error: format!("Failed to start command: {}", e),
        })?;

    Ok(child)
}

/// Execute a task with file watching, including task inputs and handling dependencies.
async fn execute_task_with_watched_files(
    task: &ExecutableTask<'_>,
    command_env: &HashMap<String, String>,
    cancel_tx: Arc<broadcast::Sender<()>>,
    ctrlc_should_exit_process: Arc<AtomicBool>,
    running_tasks: Arc<DashMap<String, Option<tokio::process::Child>>>,
    task_id: String,
    _order_idx: usize,
) -> Result<(), TaskExecutionError> {
    // Create a receiver for cancellation signals
    let mut cancel_rx = cancel_tx.subscribe();

    // Set ctrlc behavior - don't exit process on Ctrl+C during task execution
    ctrlc_should_exit_process.store(false, Ordering::Relaxed);

    // Execute the task
    let child = match execute_task(task, command_env, &mut cancel_rx).await {
        Ok(child) => child,
        Err(e) => {
            // Reset ctrlc behavior and return the error
            ctrlc_should_exit_process.store(true, Ordering::Relaxed);
            return Err(e);
        }
    };

    // Register the task in the running tasks map
    let task_name = task.name().unwrap_or("unnamed").to_string();
    let task_id_clone = task_id.clone();
    running_tasks.insert(task_id_clone, Some(child));

    // Keep track of the current child process
    let mut current_child = running_tasks
        .get_mut(&task_id)
        .and_then(|mut entry| entry.take());

    // Check for inputs to watch
    let inputs = task.task().as_execute().map_or(Vec::new(), |execute| {
        execute.inputs.as_ref().unwrap_or(&Vec::new()).clone()
    });

    if inputs.is_empty() {
        // No inputs to watch, just wait for the task to complete
        if let Some(mut child) = current_child {
            // Wait for the task to complete or be cancelled
            tokio::select! {
                status = child.wait() => {
                    // Reset ctrlc behavior
                    ctrlc_should_exit_process.store(true, Ordering::Relaxed);

                    // Remove task from running tasks
                    running_tasks.remove(&task_id);

                    match status {
                        Ok(status) => {
                            if !status.success() {
                                let code = status.code().unwrap_or(1);
                                return Err(TaskExecutionError::NonZeroExitCode(code));
                            }
                        },
                        Err(e) => {
                            tracing::error!("Error waiting for task: {}", e);
                        }
                    }
                },
                _ = cancel_rx.recv() => {
                    // Kill the process
                    let _ = kill_task(&mut child, &task_name).await;
                    // Reset ctrlc behavior
                    ctrlc_should_exit_process.store(true, Ordering::Relaxed);
                    // Remove task from running tasks
                    running_tasks.remove(&task_id);
                }
            }
        }

        return Ok(());
    }

    // Create file watcher
    let mut watcher = FileWatcher::new(&inputs).map_err(|e| {
        TaskExecutionError::InvalidWorkingDirectory(InvalidWorkingDirectory {
            path: format!("Error creating file watcher: {}", e),
        })
    })?;

    tracing::info!("Watching for changes in: {:?}", inputs);

    // For debouncing (avoid multiple rapid triggers)
    let debounce_time = Duration::from_millis(500);
    let mut last_reload = Instant::now()
        .checked_sub(debounce_time)
        .unwrap_or_else(Instant::now);

    // Main task loop
    loop {
        tokio::select! {
            // Check if current task has completed (only if we have a current task)
            status = async {
                if let Some(child) = &mut current_child {
                    child.wait().await
                } else {
                    // No current task, wait forever
                    std::future::pending::<Result<std::process::ExitStatus, std::io::Error>>().await
                }
            }, if current_child.is_some() => {
                match status {
                    Ok(status) => {
                        if !status.success() {
                            let code = status.code().unwrap_or(1);
                            tracing::error!("Task exited with non-zero status: {}", code);
                        } else {
                            tracing::info!("Task completed successfully");
                        }
                        // Task completed, reset current_child
                        current_child = None;
                    },
                    Err(e) => {
                        tracing::error!("Error waiting for task: {}", e);
                        // Something went wrong with the task, clear current_child
                        current_child = None;
                    }
                }
            },

            // Handle cancellation
            _ = cancel_rx.recv() => {
                // Kill the current task if it exists
                if let Some(mut child) = current_child.take() {
                    let _ = kill_task(&mut child, &task_name).await;
                }
                break;
            },

            // Handle file changes
            Some(event) = watcher.next_event() => {
                match event {
                    Ok(event) => {
                        match event.kind {
                            notify::event::EventKind::Create(_) |
                            notify::event::EventKind::Modify(_) |
                            notify::event::EventKind::Remove(_) => {
                                let now = Instant::now();
                                // Only reload if enough time has passed since last reload
                                if now.duration_since(last_reload) >= debounce_time {
                                    tracing::info!("Detected file change: {:?}", event.paths);
                                    last_reload = now;

                                    // Create a new cancellation receiver for the new task
                                    let mut new_cancel_rx = cancel_tx.subscribe();

                                    // Reload the task using the reload_task function
                                    if let Err(e) = reload_task(task, command_env, &mut current_child, &mut new_cancel_rx).await {
                                        tracing::error!("Error executing task after file change: {}", e);
                                    }
                                } else {
                                    tracing::debug!("Ignoring file change (debouncing): {:?}", event.paths);
                                }
                            }
                            _ => continue,
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error watching files: {}", e);
                        break;
                    }
                }
            }
        }
    }

    // Reset ctrlc behavior before returning
    ctrlc_should_exit_process.store(true, Ordering::Relaxed);

    // Make sure any remaining task is killed
    if let Some(mut child) = current_child {
        let _ = kill_task(&mut child, &task_name).await;
    }

    // Don't forget to clean up at the end
    running_tasks.remove(&task_id);

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
