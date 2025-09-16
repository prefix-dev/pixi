use std::{collections::HashMap, io::Write, path::PathBuf};

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::PathModificationBehavior,
    shell::{Bash, CmdExe, PowerShell, Shell, ShellEnum, ShellScript},
};

use pixi_config::{ConfigCli, ConfigCliActivation, ConfigCliPrompt};
use pixi_core::{
    UpdateLockFileOptions, WorkspaceLocator,
    activation::CurrentEnvVarBehavior,
    environment::{InstallFilter, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
    prompt,
    workspace::get_activated_environment_variables,
};

use crate::cli_config::{LockAndInstallConfig, WorkspaceConfig};
#[cfg(target_family = "unix")]
use pixi_pty::unix::PtySession;

use pixi_utils::prefix::Prefix;

/// Start a shell in a pixi environment, run `exit` to leave the shell.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_and_install_config: LockAndInstallConfig,

    #[clap(flatten)]
    config: ConfigCli,

    /// The environment to activate in the shell
    #[arg(long, short)]
    environment: Option<String>,

    #[clap(flatten)]
    prompt_config: ConfigCliPrompt,

    #[clap(flatten)]
    activation_config: ConfigCliActivation,
}

/// Set up Ctrl-C handler to ignore it (the child process should react on CTRL-C)
fn ignore_ctrl_c() {
    tokio::spawn(async move {
        loop {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for Ctrl+C");
            // Do nothing, effectively ignoring the Ctrl+C signal
        }
    });
}

fn start_powershell(
    pwsh: PowerShell,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(pwsh.clone(), Platform::current());
    for (key, value) in env {
        shell_script.set_env_var(key, value).into_diagnostic()?;
    }
    temp_file
        .write_all(shell_script.contents().into_diagnostic()?.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write_all(prompt.as_bytes()).into_diagnostic()?;
    temp_file.flush().into_diagnostic()?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(pwsh.executable());
    command.arg("-NoLogo");
    command.arg("-NoExit");
    command.arg("-File");
    command.arg(&temp_path);

    ignore_ctrl_c();

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

// allowing dead code so that we test this on unix compilation as well
#[allow(dead_code)]
fn start_cmdexe(
    cmdexe: CmdExe,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".cmd")
        .tempfile()
        .into_diagnostic()?;

    // TODO: Should we just execute the activation scripts directly for cmd.exe?
    let mut shell_script = ShellScript::new(cmdexe, Platform::current());
    for (key, value) in env {
        shell_script.set_env_var(key, value).into_diagnostic()?;
    }
    temp_file
        .write_all(shell_script.contents().into_diagnostic()?.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write_all(prompt.as_bytes()).into_diagnostic()?;
    temp_file.flush().into_diagnostic()?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(cmdexe.executable());
    command.arg("/K");
    command.arg(&temp_path);

    ignore_ctrl_c();

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

// allowing dead code so that we test this on unix compilation as well
#[allow(dead_code)]
fn start_winbash(
    bash: Bash,
    env: &HashMap<String, String>,
    prompt: String,
    prefix: &Prefix,
    source_shell_completions: bool,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", bash.extension()))
        .rand_bytes(3)
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(bash, Platform::current());
    for (key, value) in env {
        if key == "PATH" || key == "Path" {
            // For Git Bash on Windows, the PATH must be formatted as POSIX paths according
            // to the cygpath command, and separated by ":" instead of ";". Use the
            // shell_script.set_path call to handle these details.
            let paths = value
                .split(";")
                .map(PathBuf::from)
                .collect::<Vec<PathBuf>>();
            shell_script
                .set_path(&paths, PathModificationBehavior::Replace)
                .into_diagnostic()?;
        } else {
            shell_script.set_env_var(key, value).into_diagnostic()?;
        }
    }
    if source_shell_completions {
        if let Some(completions_dir) = bash.completion_script_location() {
            shell_script
                .source_completions(&prefix.root().join(completions_dir))
                .into_diagnostic()?;
        }
    }
    temp_file
        .write_all(shell_script.contents().into_diagnostic()?.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write_all(prompt.as_bytes()).into_diagnostic()?;
    temp_file.flush().into_diagnostic()?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(bash.executable());
    command.arg("--init-file");
    command.arg(&temp_path);
    command.arg("-i");

    ignore_ctrl_c();

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

/// Starts a UNIX shell.
/// # Arguments
/// - `shell`: The type of shell to start. Must implement the `Shell` and `Copy` traits.
/// - `args`: A vector of arguments to pass to the shell.
/// - `env`: A HashMap containing environment variables to set in the shell.
#[cfg(target_family = "unix")]
async fn start_unix_shell<T: Shell + Copy + 'static>(
    shell: T,
    args: Vec<&str>,
    env: &HashMap<String, String>,
    prompt: String,
    prefix: &Prefix,
    source_shell_completions: bool,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", shell.extension()))
        .rand_bytes(3)
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in env {
        shell_script.set_env_var(key, value).into_diagnostic()?;
    }

    if source_shell_completions {
        if let Some(completions_dir) = shell.completion_script_location() {
            shell_script
                .source_completions(&prefix.root().join(completions_dir))
                .into_diagnostic()?;
        }
    }
    const DONE_STR: &str = "PIXI_SHELL_ACTIVATION_DONE";
    shell_script.echo(DONE_STR).into_diagnostic()?;

    temp_file
        .write_all(shell_script.contents().into_diagnostic()?.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write_all(prompt.as_bytes()).into_diagnostic()?;
    temp_file.flush().into_diagnostic()?;

    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(shell.executable());
    command.args(&args);

    // Space added before `source` to automatically ignore it in history.
    let mut source_command = " ".to_string();
    shell
        .run_script(&mut source_command, temp_path.as_ref())
        .into_diagnostic()?;

    // Remove automatically added `\n`, if for some reason this fails, just ignore.
    let source_command = source_command
        .strip_suffix('\n')
        .unwrap_or(source_command.as_str());

    tracing::debug!(
        "Starting shell '{} {}' with source command: '{}'",
        shell.executable(),
        args.join(" "),
        source_command
    );

    // Start process and send env activation to the shell.
    let mut process = PtySession::new(command).into_diagnostic()?;
    process.send_line(source_command).into_diagnostic()?;

    process.interact(Some(DONE_STR)).into_diagnostic()
}

/// Starts a nu shell.
/// # Arguments
/// - `shell`: The Nushell (also contains executable location)
/// - `env`: A HashMap containing environment variables to set in the shell.
async fn start_nu_shell(
    shell: rattler_shell::shell::NuShell,
    env: &HashMap<String, String>,
    prompt: String,
) -> miette::Result<Option<i32>> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", shell.extension()))
        .rand_bytes(3)
        .tempfile()
        .into_diagnostic()?;

    let mut shell_script = ShellScript::new(shell, Platform::current());
    for (key, value) in env {
        if key == "PATH" {
            // split path with PATHSEP
            let paths = std::env::split_paths(value).collect::<Vec<_>>();
            shell_script
                .set_path(&paths, PathModificationBehavior::Replace)
                .into_diagnostic()?;
        } else {
            shell_script.set_env_var(key, value).into_diagnostic()?;
        }
    }

    temp_file
        .write_all(shell_script.contents().into_diagnostic()?.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write_all(prompt.as_bytes()).into_diagnostic()?;
    temp_file.flush().into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.arg("--execute");
    command.arg(format!("source {}", temp_file.path().display()));

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = args
        .activation_config
        .merge_config(args.prompt_config.into())
        .merge_config(args.config.clone().into());

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(config);

    let environment = workspace.environment_from_name_or_env_var(args.environment)?;

    // Make sure environment is up-to-date, default to install, users can avoid this with frozen or locked.
    #[allow(unused_variables)]
    let (lock_file_data, prefix) = get_update_lock_file_and_prefix(
        &environment,
        UpdateMode::QuickValidate,
        UpdateLockFileOptions {
            lock_file_usage: args.lock_and_install_config.lock_file_usage()?,
            no_install: args.lock_and_install_config.no_install(),
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        },
        ReinstallPackages::default(),
        &InstallFilter::default(),
    )
    .await?;
    let lock_file = lock_file_data.into_lock_file();

    // Get the environment variables we need to set activate the environment in the shell.
    let env = get_activated_environment_variables(
        workspace.env_vars(),
        &environment,
        CurrentEnvVarBehavior::Exclude,
        Some(&lock_file),
        workspace.config().force_activate(),
        workspace.config().experimental_activation_cache_usage(),
    )
    .await?;

    tracing::debug!("Pixi environment activation:\n{:?}", env);

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();

    tracing::info!("Starting shell: {:?}", interactive_shell);

    let prompt_hook = if workspace.config().change_ps1() {
        let prompt_name = prompt::prompt_name(workspace.display_name(), environment.name());
        [
            prompt::shell_prompt(&interactive_shell, prompt_name.as_str()),
            prompt::shell_hook(&interactive_shell)
                .unwrap_or_default()
                .to_owned(),
        ]
        .join("\n")
    } else {
        String::new()
    };

    let source_shell_completions = workspace.config().shell.source_completion_scripts();

    #[cfg(target_family = "windows")]
    let res = match interactive_shell {
        ShellEnum::NuShell(nushell) => start_nu_shell(nushell, env, prompt_hook).await,
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, env, prompt_hook),
        ShellEnum::CmdExe(cmdexe) => start_cmdexe(cmdexe, env, prompt_hook),
        ShellEnum::Bash(bash) => {
            start_winbash(bash, env, prompt_hook, &prefix, source_shell_completions)
        }
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell);
        }
    };

    #[cfg(target_family = "unix")]
    let res = {
        match interactive_shell {
            ShellEnum::NuShell(nushell) => start_nu_shell(nushell, env, prompt_hook).await,
            ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, env, prompt_hook),
            ShellEnum::Bash(bash) => {
                start_unix_shell(
                    bash,
                    vec!["-i"],
                    env,
                    prompt_hook,
                    &prefix,
                    source_shell_completions,
                )
                .await
            }
            ShellEnum::Zsh(zsh) => {
                start_unix_shell(
                    zsh,
                    vec!["-i"],
                    env,
                    prompt_hook,
                    &prefix,
                    source_shell_completions,
                )
                .await
            }
            ShellEnum::Fish(fish) => {
                start_unix_shell(
                    fish,
                    vec![],
                    env,
                    prompt_hook,
                    &prefix,
                    source_shell_completions,
                )
                .await
            }
            ShellEnum::Xonsh(xonsh) => {
                start_unix_shell(
                    xonsh,
                    vec![],
                    env,
                    prompt_hook,
                    &prefix,
                    source_shell_completions,
                )
                .await
            }
            _ => {
                miette::bail!("Unsupported shell: {:?}", interactive_shell)
            }
        }
    };

    match res {
        Ok(Some(code)) => std::process::exit(code),
        Ok(None) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error starting shell: {}", e);
            std::process::exit(1);
        }
    }
}
