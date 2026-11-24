use std::{
    collections::{HashMap, hash_map::Entry},
    ffi::OsString,
    rc::Rc,
    sync::Arc,
};

#[cfg(unix)]
use std::io::IsTerminal;

use deno_task_shell::{KillSignal, ShellCommand};
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_core::{
    Workspace,
    activation::ActivationError,
    environment::{LockFileUsage, SanityCheckError, sanity_check_workspace},
    lock_file::{
        LockFileDerivedData, ReinstallPackages, UpdateLockFileError, UpdateLockFileOptions,
        UpdateMode, UpdatePrefixError,
    },
    workspace::{
        Environment, EnvironmentFromNameError,
        platform_selection::{PrefixPolicy, ResolvedPlatform},
        virtual_packages::{
            RunPlatformUnsupportedError, environment_has_dependencies_for_platform,
            verify_resolved_platform,
        },
    },
};
use pixi_manifest::{EnvironmentName, PixiPlatformName};
use pixi_task::{
    AmbiguousTask, CacheUpdateError, CanSkip, DisambiguationSource, ExecutableTask,
    FailedToParseShellScript, InputHashesError, InvalidWorkingDirectory, PreferExecutable,
    SearchEnvironments, TaskAndEnvironment, TaskGraph, TaskGraphError, get_task_env,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::Level;

use crate::interface::Interface;

#[derive(Default, Serialize, Deserialize)]
pub struct RunOptions {
    /// The pixi task or a task shell command to run.
    pub task: Vec<String>,

    /// The environment to run the task in.
    pub environment: Option<EnvironmentName>,

    /// The workspace platform to install and run the environment for.
    pub platform: Option<PixiPlatformName>,

    /// Execute the command as an executable without resolving Pixi tasks.
    pub executable: bool,

    /// Use a clean environment to run the task.
    pub clean_env: bool,

    /// Don't run the dependencies of the task.
    pub skip_deps: bool,

    /// Enable template rendering for the command arguments.
    pub templated: bool,

    /// Only print the commands that would run.
    pub dry_run: bool,

    pub lock_file_usage: LockFileUsage,

    pub no_install: bool,

    /// Additional commands available to the task shell.
    #[serde(skip)]
    pub custom_commands: HashMap<String, Rc<dyn ShellCommand>>,

    /// Shown instead of the task's command in the task header.
    pub display_command: Option<String>,
}
/// The result of a run and the authoritative selections used by task lookup and execution.
#[derive(Debug)]
pub struct RunOutcome<'p> {
    pub exit_code: i32,
    pub platforms: HashMap<EnvironmentName, ResolvedPlatform<'p>>,
}

/// An error that can occur while running a task or command.
#[derive(Debug, Error, Diagnostic)]
pub enum RunError {
    #[error("no task or command provided")]
    NoTask,

    #[error(transparent)]
    #[diagnostic(transparent)]
    EnvironmentFromName(#[from] EnvironmentFromNameError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SanityCheck(#[from] SanityCheckError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    UpdateLockFile(#[from] UpdateLockFileError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    TaskGraph(#[from] TaskGraphError),

    #[error(transparent)]
    CheckCache(std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    UpdatePrefix(#[from] UpdatePrefixError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RunPlatformUnsupported(#[from] RunPlatformUnsupportedError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Activation(#[from] ActivationError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    FailedToParseShellScript(#[from] FailedToParseShellScript),

    #[error(transparent)]
    InvalidWorkingDirectory(#[from] InvalidWorkingDirectory),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InputHashes(#[from] InputHashesError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    CacheUpdate(#[from] CacheUpdateError),
}

pub async fn run<'p, I: Interface>(
    interface: &I,
    workspace: &'p Workspace,
    options: RunOptions,
    progress: Option<&Arc<pixi_reporters::TopLevelProgress>>,
) -> Result<RunOutcome<'p>, RunError> {
    // Fail before resolving the lock file, there is nothing to run
    if options.task.iter().all(|arg| arg.trim().is_empty()) {
        return Err(RunError::NoTask);
    }

    let explicit_environment = options
        .environment
        .as_ref()
        .map(|name| {
            workspace
                .environment(name)
                .ok_or_else(|| EnvironmentFromNameError::Unknown(name.clone()))
        })
        .transpose()?;
    let allow_installs = !options.no_install;

    // Sanity check of prefix location
    sanity_check_workspace(workspace).await?;

    let user_platform = options.platform.clone();

    // Top-level progress, kept here so we can clear it between phases.
    let progress = progress.cloned();

    // Ensure that the lock file is up-to-date.
    let mut lock_file = workspace
        .resolve_lock_file(
            progress.clone(),
            UpdateLockFileOptions {
                lock_file_usage: options.lock_file_usage,
                no_install: options.no_install,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
                ..Default::default()
            },
        )
        .await?
        .0;

    // Only an explicit `--platform` pins the global target. Task graph
    // construction detects each relevant environment's host before selecting
    // its platform and resolving platform-specific task definitions.
    lock_file.set_explicit_target_platform(user_platform.clone());

    // Spawn a task that listens for ctrl+c and resets the cursor.
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            reset_cursor();
        }
    });

    // Construct a task graph from the input arguments.
    let (task_graph, platforms) = build_task_graph(
        interface,
        workspace,
        explicit_environment,
        &options,
        &lock_file,
    )
    .await?;
    tracing::debug!("Task graph: {}", task_graph);

    if options.dry_run {
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

    let init_cwd = std::env::current_dir().ok();
    for task_id in task_graph.topological_order() {
        let resolved = platforms
            .get(task_graph[task_id].run_environment.name())
            .expect("task graph environments have a resolved platform");
        let executable_task = ExecutableTask::from_task_graph(
            &task_graph,
            task_id,
            init_cwd.clone(),
            resolved.platform(),
        );

        // If the task is not executable (e.g. an alias), we skip it. This ensures we
        // don't instantiate a prefix for an alias.
        if !executable_task.task().is_executable() {
            continue;
        }

        // Showing which command is being run if the level and type allows it.
        if tracing::enabled!(Level::WARN)
            && (!executable_task.task().is_custom() || options.display_command.is_some())
        {
            if task_idx > 0 {
                pixi_progress::println!();
            }

            let display_command = if let Some(display_command) = &options.display_command {
                display_command.clone()
            } else {
                executable_task.display_command().to_string()
            };

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
                    console::style(format!(": ({description})")).yellow()
                } else {
                    console::style("".to_string()).yellow()
                }
            );
        }

        // on dry-run mode, we just print the command and skip the execution
        if options.dry_run {
            task_idx += 1;
            continue;
        }

        // check task cache
        let task_cache = match executable_task
            .can_skip(lock_file.as_lock_file())
            .await
            .map_err(RunError::CheckCache)?
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
        let task_env: &_ = match task_envs.entry(executable_task.run_environment.name().clone()) {
            Entry::Occupied(env) => env.into_mut(),
            Entry::Vacant(entry) => {
                // Report the platform per environment: a bare `pixi run` may
                // span environments that declare different platforms.
                tracing::info!(
                    "Running tasks in environment '{}' assuming platform '{}'",
                    executable_task.run_environment.name().fancy_display(),
                    executable_task.platform.name(),
                );

                if allow_installs
                    && (executable_task
                        .run_environment
                        .installed_resolved_platform()
                        .is_some()
                        || environment_has_dependencies_for_platform(
                            &executable_task.run_environment,
                            resolved.platform(),
                        ))
                {
                    lock_file
                        .prefix(
                            &executable_task.run_environment,
                            resolved,
                            UpdateMode::QuickValidate,
                            &ReinstallPackages::default(),
                            &pixi_core::environment::InstallFilter::default(),
                        )
                        .await?;
                }
                verify_resolved_platform(&executable_task.run_environment, resolved)?;
                executable_task
                    .run_environment
                    .emit_emulation_warning(resolved.platform());

                if let Some(progress) = &progress {
                    progress.on_clear();
                }

                // Clear caches based on the filesystem. The tasks might change files on disk.
                lock_file.command_dispatcher.clear_filesystem_caches().await;

                let command_env = get_task_env(
                    &executable_task.run_environment,
                    resolved.platform(),
                    options.clean_env || executable_task.task().clean_env(),
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
        // failed with a non-zero exit code, we return the same code.
        let code = execute_task(
            &executable_task,
            &task_env,
            signal.clone(),
            &options.custom_commands,
        )
        .await?;
        if code != 0 {
            return Ok(RunOutcome {
                exit_code: code,
                platforms,
            });
        }
        task_idx += 1;

        // Compute post-run hash, warn on missing globs, and update the cache
        let post_hash = executable_task
            .compute_post_run_hash(lock_file.as_lock_file(), task_cache)
            .await?;
        if let Some(ref hash) = post_hash {
            executable_task.warn_on_missing_globs(hash);
        }
        executable_task.save_cache(post_hash).await?;
    }

    Ok(RunOutcome {
        exit_code: 0,
        platforms,
    })
}

async fn build_task_graph<'p, I: Interface>(
    interface: &I,
    workspace: &'p Workspace,
    explicit_environment: Option<Environment<'p>>,
    options: &RunOptions,
    lock_file: &LockFileDerivedData<'p>,
) -> Result<
    (
        TaskGraph<'p>,
        HashMap<EnvironmentName, ResolvedPlatform<'p>>,
    ),
    TaskGraphError,
> {
    let mut search_environment = SearchEnvironments::from_opt_env(
        workspace,
        explicit_environment,
        options.platform.clone(),
        async |environment: &Environment<'p>| {
            lock_file
                .resolve_platform(
                    environment,
                    if options.no_install {
                        PrefixPolicy::Reuse
                    } else {
                        PrefixPolicy::Replace
                    },
                    UpdateMode::QuickValidate,
                )
                .await
        },
    )
    .with_disambiguate_fn(async |problem: &AmbiguousTask<'p>| {
        disambiguate_task_interactive(interface, problem).await
    });

    let graph = TaskGraph::from_cmd_args(
        workspace,
        &mut search_environment,
        options.task.clone(),
        options.skip_deps,
        if options.executable {
            PreferExecutable::Always
        } else {
            PreferExecutable::TaskFirst
        },
        options.templated,
    )
    .await?;
    Ok((graph, search_environment.into_platforms()))
}

/// Called to execute a single command. Returns its exit code.
async fn execute_task(
    task: &ExecutableTask<'_, '_>,
    command_env: &HashMap<OsString, OsString>,
    kill_signal: KillSignal,
    custom_commands: &HashMap<String, Rc<dyn ShellCommand>>,
) -> Result<i32, RunError> {
    let Some(script) = task.as_deno_script()? else {
        return Ok(0);
    };
    let cwd = task.working_directory()?;
    let execute_future = deno_task_shell::execute(
        script,
        command_env.clone(),
        cwd,
        custom_commands.clone(),
        kill_signal.clone(),
    );

    // Execute the process and forward signals.
    Ok(run_future_forwarding_signals(kill_signal, execute_future).await)
}

/// Called to disambiguate between environments to run a task in.
async fn disambiguate_task_interactive<'p, I: Interface>(
    interface: &I,
    problem: &AmbiguousTask<'p>,
) -> Result<Option<TaskAndEnvironment<'p>>, DisambiguationSource> {
    // If any of the candidate tasks declares a `default-environment` that
    // corresponds to one of the candidate environments, prefer that
    // environment automatically.
    if let Some(idx) = problem.environments.iter().position(|(env, task)| {
        if let Some(default_env_name) = task.default_environment() {
            default_env_name == env.name()
        } else {
            false
        }
    }) {
        return Ok(Some(problem.environments[idx].clone()));
    }

    let environment_names = problem
        .environments
        .iter()
        .map(|(env, _)| env.name().to_string())
        .collect_vec();

    let selected = interface
        .select(
            &format!(
                "The task '{}' {}can be run in multiple environments.\n\nPlease select an environment to run the task in:",
                problem.task_name.fancy_display(),
                if let Some(dependency) = &problem.depended_on_by {
                    format!("(depended on by '{}') ", dependency.0.fancy_display())
                } else {
                    String::new()
                }
            ),
            &environment_names,
        )
        .await?;
    Ok(selected.map(|idx| problem.environments[idx].clone()))
}

/// `dialoguer` doesn't clean up your term if it's aborted via e.g. `SIGINT` or
/// other exceptions: <https://github.com/console-rs/dialoguer/issues/188>.
///
/// `dialoguer`, as a library, doesn't want to mess with signal handlers,
/// but we, as an application, are free to mess with signal handlers if we feel
/// like it, since we own the process.
/// This function was taken from <https://github.com/dnjstrom/git-select-branch/blob/16c454624354040bc32d7943b9cb2e715a5dab92/src/main.rs#L119>.
fn reset_cursor() {
    let term = console::Term::stdout();
    let _ = term.show_cursor();
}

/// Runs a task future forwarding any signals received to the process.
///
/// Signal listeners and ctrl+c listening will be setup.
pub async fn run_future_forwarding_signals<TOutput>(
    #[cfg_attr(windows, allow(unused_variables))] kill_signal: KillSignal,
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
            #[cfg(windows)]
            spawn_future_with_cancellation(listen_ctrl_c_windows(), token.clone());

            #[cfg(unix)]
            spawn_future_with_cancellation(listen_and_forward_all_signals(kill_signal), token);

            future.await
        })
        .await
}

#[cfg(windows)]
async fn listen_ctrl_c_windows() {
    // On windows, ctrl+c is sent to the process group, so the signal would
    // have already been sent to the child process. We still want to listen
    // for ctrl+c here to keep the process alive when receiving it, but no
    // need to forward the signal because it's already been sent.
    while let Ok(()) = tokio::signal::ctrl_c().await {}
}

/// Listens to all incoming signals and forwards all of them, except
/// some cases.
///
/// Note that we don't handle `SIGINT` correctly, if the subprocess changes
/// its PGID, then the system won't forward CTRL+C automatically.
/// However, we should do that to ensure consistent behaviour.
///
/// To resolve this we should patch `deno_task_shell` to return PID
/// from which we could get PGID and do things right.
///
/// Resulting approach should mimic
/// <https://github.com/astral-sh/uv/blob/9d17dfa3537312b928f94479f632891f918c4760/crates/uv/src/child.rs#L156C21-L168C77>
#[cfg(unix)]
async fn listen_and_forward_all_signals(kill_signal: KillSignal) {
    use futures::FutureExt;

    use pixi_core::signals::SIGNALS;

    // listen and forward every signal we support
    let mut futures = Vec::with_capacity(SIGNALS.len());
    let is_interactive = std::io::stdin().is_terminal();
    for signo in SIGNALS.iter().copied() {
        if signo == libc::SIGKILL
            || signo == libc::SIGSTOP
            || (signo == libc::SIGINT && is_interactive)
        {
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
