use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    convert::identity,
    ffi::OsString,
    string::String,
};

use clap::Parser;
use deno_task_shell::KillSignal;
use dialoguer::theme::ColorfulTheme;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_config::{ConfigCli, ConfigCliActivation};
use pixi_core::{
    Workspace, WorkspaceLocator,
    environment::sanity_check_workspace,
    lock_file::{ReinstallPackages, UpdateLockFileOptions, UpdateMode},
    workspace::{Environment, errors::UnsupportedPlatformError},
};
use pixi_manifest::{FeaturesExt, TaskName};
use pixi_task::{
    AmbiguousTask, CanSkip, ExecutableTask, FailedToParseShellScript, InvalidWorkingDirectory,
    SearchEnvironments, TaskAndEnvironment, TaskGraph, get_task_env,
};
use rattler_conda_types::Platform;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Level;

use crate::cli_config::{LockAndInstallConfig, WorkspaceConfig};

/// Runs task in the pixi environment.
///
/// This command is used to run tasks in the pixi environment.
/// It will activate the environment and run the task in the environment.
/// It is using the deno_task_shell to run the task.
///
/// `pixi run` will also update the lockfile and install the environment if it
/// is required.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, disable_help_flag = true)]
pub struct Args {
    /// The pixi task or a task shell command you want to run in the workspace's
    /// environment, which can be an executable in the environment's PATH.
    pub task: Vec<String>,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

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

/// CLI entry point for `pixi run`
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
    sanity_check_workspace(&workspace).await?;

    let best_platform = environment.best_platform();

    // Ensure that the lock-file is up-to-date.
    let lock_file = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_and_install_config.lock_file_usage()?,
            no_install: args.lock_and_install_config.no_install(),
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await?
        .0;

    // Spawn a task that listens for ctrl+c and resets the cursor.
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            reset_cursor();
        }
    });

    // Construct a task graph from the input arguments
    let search_environment = SearchEnvironments::from_opt_env(
        &workspace,
        explicit_environment.clone(),
        Some(best_platform),
    )
    .with_disambiguate_fn(disambiguate_task_interactive);

    let task_graph =
        TaskGraph::from_cmd_args(&workspace, &search_environment, args.task, args.skip_deps)?;

    tracing::debug!("Task graph: {}", task_graph);

    // Print dry-run message if dry-run mode is enabled
    if args.dry_run {
        pixi_progress::println!(
            "{}{}\n\n",
            console::Emoji("🌵 ", ""),
            console::style("Dry-run mode enabled - no tasks will be executed.")
                .yellow()
                .bold()
        );
    }

    // Traverse the task graph in topological order and execute each individual
    // task.
    let mut task_idx = 0;
    let mut task_envs = HashMap::new();
    let signal = KillSignal::default();
    // make sure that child processes are killed when pixi stops
    let _drop_guard = signal.clone().drop_guard();

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
                pixi_progress::println!();
            }

            let display_command = executable_task.display_command().to_string();

            pixi_progress::println!(
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
                display_command,
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
            .can_skip(lock_file.as_lock_file())
            .await
            .into_diagnostic()?
        {
            CanSkip::No(cache) => cache,
            CanSkip::Yes => {
                let args_text = if !executable_task.args().is_empty() {
                    format!(
                        " with args {}",
                        console::style(executable_task.args()).bold()
                    )
                } else {
                    String::new()
                };

                pixi_progress::println!(
                    "Task '{}'{args_text} can be skipped (cache hit) 🚀",
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
                // Check if we allow installs
                if args.lock_and_install_config.allow_installs() {
                    // Ensure there is a valid prefix
                    lock_file
                        .prefix(
                            &executable_task.run_environment,
                            UpdateMode::QuickValidate,
                            &ReinstallPackages::default(),
                            &pixi_core::environment::InstallFilter::default(),
                        )
                        .await?;
                }

                // Clear the current progress reports.
                lock_file.command_dispatcher.clear_reporter().await;

                let command_env = get_task_env(
                    &executable_task.run_environment,
                    args.clean_env || executable_task.task().clean_env(),
                    Some(lock_file.as_lock_file()),
                    workspace.config().force_activate(),
                    workspace.config().experimental_activation_cache_usage(),
                )
                .await?;
                entry.insert(command_env)
            }
        };

        let task_env = task_env
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect();

        // Execute the task itself within the command environment. If one of the tasks
        // failed with a non-zero exit code, we exit this parent process with
        // the same code.
        match execute_task(&executable_task, &task_env, signal.clone()).await {
            Ok(_) => {
                task_idx += 1;
            }
            Err(TaskExecutionError::NonZeroExitCode(code)) => {
                if code == 127 {
                    command_not_found(&workspace, explicit_environment.clone());
                }
                std::process::exit(code);
            }
            Err(err) => return Err(err.into()),
        }

        // Update the task cache with the new hash
        executable_task
            .save_cache(lock_file.as_lock_file(), task_cache)
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
        pixi_progress::println!(
            "\nAvailable tasks:\n{}",
            available_tasks
                .into_iter()
                .sorted()
                .format_with("\n", |name, f| {
                    f(&format_args!("\t{}", name.fancy_display().bold()))
                })
        );
    }

    // Help user when there is no task available because the platform is not
    // supported
    if workspace
        .environments()
        .iter()
        .all(|env| !env.platforms().contains(&env.best_platform()))
    {
        pixi_progress::println!(
            "\nHelp: This platform ({}) is not supported. Please run the following command to add this platform to the workspace:\n\n\tpixi workspace platform add {}",
            Platform::current(),
            Platform::current()
        );
    }
}

#[derive(Debug, Error, Diagnostic)]
enum TaskExecutionError {
    #[error("the script exited with a non-zero exit code {0}")]
    NonZeroExitCode(i32),

    #[error(transparent)]
    #[diagnostic(transparent)]
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
    command_env: &HashMap<OsString, OsString>,
    kill_signal: KillSignal,
) -> Result<(), TaskExecutionError> {
    let Some(script) = task.as_deno_script()? else {
        return Ok(());
    };
    let cwd = task.working_directory()?;
    let execute_future = deno_task_shell::execute(
        script,
        command_env.clone(),
        cwd,
        Default::default(),
        kill_signal.clone(),
    );

    // Execute the process and forward signals.
    let status_code = run_future_forwarding_signals(kill_signal, execute_future).await;
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

// /// Exit the process with the appropriate exit code for a SIGINT.
// fn exit_process_on_sigint() {
//     // https://learn.microsoft.com/en-us/cpp/c-runtime-library/signal-constants
//     #[cfg(target_os = "windows")]
//     std::process::exit(3);
//
//     // POSIX compliant OSs: 128 + SIGINT (2)
//     #[cfg(not(target_os = "windows"))]
//     std::process::exit(130);
// }

/// Runs a task future forwarding any signals received to the process.
///
/// Signal listeners and ctrl+c listening will be setup.
pub async fn run_future_forwarding_signals<TOutput>(
    kill_signal: KillSignal,
    future: impl std::future::Future<Output = TOutput>,
) -> TOutput {
    fn spawn_future_with_cancellation(
        future: impl std::future::Future<Output = ()> + 'static,
        token: CancellationToken,
    ) {
        tokio::task::spawn_local(async move {
            tokio::select! {
              _ = future => {}
              _ = token.cancelled() => {}
            }
        });
    }

    let token = CancellationToken::new();
    let _token_drop_guard = token.clone().drop_guard();
    let local_set = tokio::task::LocalSet::new();

    local_set
        .run_until(async move {
            spawn_future_with_cancellation(listen_ctrl_c(kill_signal.clone()), token.clone());
            #[cfg(unix)]
            spawn_future_with_cancellation(listen_and_forward_all_signals(kill_signal), token);

            future.await
        })
        .await
}

async fn listen_ctrl_c(kill_signal: KillSignal) {
    while let Ok(()) = tokio::signal::ctrl_c().await {
        // On windows, ctrl+c is sent to the process group, so the signal would
        // have already been sent to the child process. We still want to listen
        // for ctrl+c here to keep the process alive when receiving it, but no
        // need to forward the signal because it's already been sent.
        if !cfg!(windows) {
            kill_signal.send(deno_task_shell::SignalKind::SIGINT)
        }
    }
}

#[cfg(unix)]
async fn listen_and_forward_all_signals(kill_signal: KillSignal) {
    use futures::FutureExt;

    use pixi_core::signals::SIGNALS;

    // listen and forward every signal we support
    let mut futures = Vec::with_capacity(SIGNALS.len());
    for signo in SIGNALS.iter().copied() {
        if signo == libc::SIGKILL || signo == libc::SIGSTOP {
            continue; // skip, can't listen to these
        }

        let kill_signal = kill_signal.clone();
        futures.push(
            async move {
                let Ok(mut stream) = tokio::signal::unix::signal(signo.into()) else {
                    return;
                };
                let signal_kind = signo.into();
                while let Some(()) = stream.recv().await {
                    kill_signal.send(signal_kind);
                }
            }
            .boxed_local(),
        )
    }
    futures::future::join_all(futures).await;
}
