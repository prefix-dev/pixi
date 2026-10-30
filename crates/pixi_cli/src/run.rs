use std::{
    collections::{HashMap, HashSet},
    io::Read,
    process::ExitCode,
};

use clap::Parser;
use fancy_display::FancyDisplay;
use indicatif::ProgressDrawTarget;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_api::workspace::RunOptions;
use pixi_config::{ConfigCli, ConfigCliActivation};
use pixi_core::host::{cli_detector_consent, probe_conda_script, probe_pep723_script};

use pixi_core::{
    Workspace, WorkspaceLocator,
    workspace::{
        Environment,
        virtual_packages::{EnvironmentRunnability, classify_environment_runnability},
    },
};
use pixi_manifest::{PixiPlatformName, TaskName, WithWarnings};
use pixi_progress::global_multi_progress;
use rattler_conda_types::Subdir;

use crate::cli_config::{
    LockAndInstallConfig, ScriptWorkspaceConfig, validate_transient_script_lock_file_usage,
};
use crate::cli_interface::cli_context;
use crate::process_exit;
use crate::run_script::{
    RemoteScriptManifest, RunScriptInput, STDIN_SCRIPT_COMMAND, prepare_remote_script,
    prepare_stdin_script, transient_script_cache_key,
};
use crate::shared::install_platform::resolve_install_platform;

/// Run a task in an environment.
///
/// This command is used to run tasks in the pixi environment.
/// It will activate the environment and run the task in the environment.
/// It is using the deno_task_shell to run the task.
///
/// `pixi run` will also update the lock file and install the environment if it
/// is required.
#[derive(Parser, Debug, Default)]
#[clap(trailing_var_arg = true, disable_help_flag = true)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    /// The pixi task or a task shell command you want to run in the workspace's
    /// environment, which can be an executable in the environment's PATH.
    pub task: Vec<String>,

    /// Execute the command as an executable without resolving Pixi tasks.
    ///
    /// Useful when a task name and an executable have the same name.
    #[arg(long = "executable", short = 'x')]
    pub executable: bool,

    /// Enable experimental `--script` features; currently the `conda-script`
    /// block.
    ///
    /// The flag has no effect for PEP 723 scripts, so a shebang line can
    /// pass it unconditionally. Setting `experimental.conda-script` in the
    /// configuration opts in without the flag, with a warning on every run.
    #[arg(long, requires = "script")]
    pub experimental: bool,

    #[clap(flatten)]
    pub workspace_config: ScriptWorkspaceConfig,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub activation_config: ConfigCliActivation,

    /// The environment to run the task in.
    #[arg(long, short)]
    pub environment: Option<String>,

    /// Install and run in the environment for the given platform; a warning is
    /// printed when it doesn't run on this machine. Accepts a workspace
    /// platform name; a bare conda subdir (e.g. `linux-64`) is also accepted.
    #[arg(long, short)]
    pub platform: Option<PixiPlatformName>,

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

    /// Enable template rendering for the command arguments.
    ///
    /// By default, arguments passed to `pixi run` on the command line are not
    /// processed by the template engine. Use this flag to enable rendering
    /// of template variables like `{{ pixi.platform }}`.
    #[arg(long)]
    pub templated: bool,

    /// Run the task in dry-run mode (only print the command that would run)
    #[clap(short = 'n', long)]
    pub dry_run: bool,

    #[clap(long, action = clap::ArgAction::HelpLong)]
    pub help: Option<bool>,

    #[clap(short, action = clap::ArgAction::HelpShort)]
    pub h: Option<bool>,
}

impl Args {
    fn validate_script_options(&self) -> miette::Result<()> {
        if self.workspace_config.script.is_none() {
            return Ok(());
        }

        let mut unsupported = Vec::new();
        if self.environment.is_some() {
            unsupported.push("--environment");
        }
        if self.skip_deps {
            unsupported.push("--skip-deps");
        }

        if unsupported.is_empty() {
            Ok(())
        } else {
            Err(miette::miette!(
                help = "A PEP 723 script has one implicit default run environment and no Pixi task graph.",
                "`pixi run --script` does not support {}",
                unsupported.join(", ")
            ))
        }
    }
}

/// CLI entry point for `pixi run`
/// When running the sigints are ignored and child can react to them. As it
/// pleases.
pub async fn execute(mut args: Args) -> miette::Result<ExitCode> {
    args.validate_script_options()?;

    // Following statements don't spawn any progress bar, so set
    // progress draw target to hidden. Otherwise output may be
    // incorrect.
    let not_hidden = !global_multi_progress().is_hidden();
    global_multi_progress().set_draw_target(ProgressDrawTarget::hidden());

    let cli_config = args
        .activation_config
        .clone()
        .merge_config(args.config.clone().into());

    let is_script = args.workspace_config.script.is_some();
    let script_input = args
        .workspace_config
        .script
        .as_deref()
        .map(RunScriptInput::classify);
    let requested_lock_file_usage = args.lock_and_install_config.lock_file_usage()?;
    let global_config_source = args.config_source.source();
    let mut _remote_script_directory = None;
    let mut stdin_script_command = None;
    let workspace = match script_input {
        Some(RunScriptInput::Remote(url)) => {
            validate_transient_script_lock_file_usage(requested_lock_file_usage)?;
            let root = std::env::current_dir().into_diagnostic()?;
            let config = pixi_config::Config::try_load_with(&root, &global_config_source)?
                .merge_config(cli_config);
            let prepared = prepare_remote_script(url, &config, &root, args.experimental).await?;
            let cache_key =
                transient_script_cache_key(b"remote", prepared.original_url.as_str().as_bytes());
            let manifest = match prepared.manifest {
                RemoteScriptManifest::Pep723(manifest) => manifest,
                RemoteScriptManifest::CondaScript(manifest) => {
                    let entrypoint = manifest.metadata().entrypoint.clone();
                    let host =
                        probe_conda_script(&manifest, &root, &config, cli_detector_consent(None))
                            .await?;
                    let workspace = Workspace::from_transient_conda_script(
                        manifest,
                        config,
                        root,
                        &prepared.cache_name,
                        &cache_key,
                        host,
                    )?;
                    if not_hidden {
                        global_multi_progress()
                            .set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
                    }
                    let code =
                        crate::conda_script::execute_run(workspace, entrypoint, args).await?;
                    drop(prepared.directory);
                    return Ok(process_exit::exit_code_from_code(code));
                }
            };
            let script_path = manifest.path().to_owned();
            let host =
                probe_pep723_script(&manifest, &root, &config, cli_detector_consent(None)).await?;
            let WithWarnings {
                value: workspace,
                warnings,
            } = Workspace::from_transient_script(
                manifest,
                config,
                root,
                script_path,
                &prepared.cache_name,
                &cache_key,
                host,
            )?;
            for warning in warnings {
                tracing::warn!("{warning}");
            }
            _remote_script_directory = Some(prepared.directory);
            workspace
        }
        Some(RunScriptInput::Stdin) => {
            validate_transient_script_lock_file_usage(requested_lock_file_usage)?;
            let root = std::env::current_dir().into_diagnostic()?;
            let config = pixi_config::Config::try_load_with(&root, &global_config_source)?
                .merge_config(cli_config);
            let mut contents = Vec::new();
            std::io::stdin()
                .read_to_end(&mut contents)
                .into_diagnostic()?;
            let prepared = prepare_stdin_script(contents, &root)?;
            let cache_key =
                transient_script_cache_key(b"stdin", prepared.manifest.metadata().as_bytes());
            let host = probe_pep723_script(
                &prepared.manifest,
                &root,
                &config,
                cli_detector_consent(None),
            )
            .await?;
            let WithWarnings {
                value: workspace,
                warnings,
            } = Workspace::from_transient_script(
                prepared.manifest,
                config,
                root,
                "<stdin>".into(),
                "stdin",
                &cache_key,
                host,
            )?;
            for warning in warnings {
                tracing::warn!("{warning}");
            }
            stdin_script_command = Some(prepared.command);
            workspace
        }
        Some(RunScriptInput::Local(path)) => {
            let root = std::path::absolute(&path)
                .into_diagnostic()?
                .parent()
                .expect("an absolute script path always has a parent")
                .to_owned();
            let config = pixi_config::Config::try_load_with(&root, &global_config_source)?
                .merge_config(cli_config.clone());
            // The configuration option opts in for every run, so the flag is
            // only needed when it is unset.
            let opted_in = args.experimental || config.experimental_conda_script();
            // A conda-script block takes this file off the PEP 723 path; a
            // file with both kinds of block is rejected by the detection.
            if let Some(manifest) = crate::conda_script::detect_with_fallback(&path, opted_in)? {
                crate::conda_script::ensure_enabled(
                    args.experimental,
                    &config,
                    &path.display().to_string(),
                )?;
                if not_hidden {
                    global_multi_progress().set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
                }
                let entrypoint = manifest.metadata().entrypoint.clone();
                let host =
                    probe_conda_script(&manifest, &root, &config, cli_detector_consent(None))
                        .await?;
                let workspace = Workspace::from_conda_script(manifest, config, host)?;
                let code = crate::conda_script::execute_run(workspace, entrypoint, args).await?;
                return Ok(process_exit::exit_code_from_code(code));
            }
            WorkspaceLocator::for_cli()
                .with_global_config_source(global_config_source)
                .with_search_start(pixi_core::workspace::DiscoveryStart::Script(path))
                .with_cli_config(cli_config)
                .locate()
                .await?
        }
        None => {
            WorkspaceLocator::for_cli()
                .with_global_config_source(global_config_source)
                .with_search_start(args.workspace_config.workspace_locator_start())
                .with_cli_config(cli_config)
                .locate()
                .await?
        }
    };

    let stdin_display_args = if stdin_script_command.is_some() {
        Some(args.task.clone())
    } else {
        None
    };
    if stdin_script_command.is_some() {
        args.task.insert(0, STDIN_SCRIPT_COMMAND.to_owned());
        args.executable = true;
    } else if is_script {
        let script_path = workspace.workspace.provenance.path.clone();
        let script_path = script_path.into_os_string().into_string().map_err(|_| {
            miette::miette!("the script path must contain only valid UTF-8 characters")
        })?;

        args.task.insert(0, script_path);
        args.task.insert(0, "python".to_owned());
        args.executable = true;
    }

    // Extract the passed in environment name.
    let environment = if is_script {
        workspace.default_environment()
    } else {
        workspace.environment_from_name_or_env_var(args.environment.clone())?
    };

    // Find the environment to run the task in, if any were specified.
    let explicit_environment =
        if is_script || (args.environment.is_none() && environment.is_default()) {
            None
        } else {
            Some(environment.clone())
        };

    // Print all available tasks if no task is provided
    if args.task.is_empty() {
        command_not_found(&workspace, explicit_environment);
        return Ok(ExitCode::SUCCESS);
    }

    // We expect progress bar to be used afterwards, so set draw
    // target to the original one.
    if not_hidden {
        global_multi_progress().set_draw_target(ProgressDrawTarget::stderr_with_hz(20));
    }

    let options = RunOptions {
        task: args.task,
        environment: explicit_environment.as_ref().map(|env| env.name().clone()),
        platform: resolve_install_platform(&workspace, args.platform.as_ref())?,
        executable: args.executable,
        clean_env: args.clean_env,
        skip_deps: args.skip_deps,
        templated: args.templated,
        dry_run: args.dry_run,
        lock_file_usage: requested_lock_file_usage,
        no_install: args.lock_and_install_config.no_install(),
        custom_commands: stdin_script_command
            .map(|command| {
                HashMap::from([(STDIN_SCRIPT_COMMAND.to_owned(), command.shell_command())])
            })
            .unwrap_or_default(),
        display_command: stdin_display_args.map(|forwarded_args| {
            if forwarded_args.is_empty() {
                "python -c <stdin>".to_owned()
            } else {
                format!("python -c <stdin> {}", forwarded_args.iter().format(" "))
            }
        }),
    };
    let code = cli_context(workspace.clone()).run(options).await?;

    if code == 127 {
        command_not_found(&workspace, explicit_environment);
    }
    Ok(process_exit::exit_code_from_code(code))
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

    // Point at the missing platform only when it is genuinely what blocks the
    // run. An environment that installs nothing, or whose packages the machine
    // already satisfies, runs here regardless of the declared platforms, so
    // suggesting `platform add` would send the user after the wrong problem.
    if workspace.environments().iter().all(|env| {
        classify_environment_runnability(env, None) == EnvironmentRunnability::Unsupported
    }) {
        pixi_progress::println!(
            "\nHelp: This platform ({}) is not supported. Please run the following command to add this platform to the workspace:\n\n\tpixi workspace platform add {}",
            Subdir::current().unwrap_or(Subdir::NoArch),
            Subdir::current().unwrap_or(Subdir::NoArch)
        );
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Args;

    #[test]
    fn experimental_requires_a_script() {
        assert!(Args::try_parse_from(["run", "--experimental", "--script", "main.c"]).is_ok());
        assert!(Args::try_parse_from(["run", "--experimental", "task"]).is_err());
    }
}
