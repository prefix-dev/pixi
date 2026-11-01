use std::{collections::HashMap, io::Write, path::PathBuf, sync::Arc};

use miette::Diagnostic;
use rattler_conda_types::Subdir;
use rattler_shell::{
    activation::PathModificationBehavior,
    shell::{Bash, CmdExe, PowerShell, Shell, ShellEnum, ShellScript},
};
use which::which;

use pixi_core::{
    UpdateLockFileOptions, Workspace,
    activation::{ActivationError, CurrentEnvVarBehavior},
    environment::{
        InstallFilter, LockFileAndPrefixesError, LockFileUsage, get_update_lock_file_and_prefix,
    },
    lock_file::{ReinstallPackages, UpdateMode},
    prompt,
    workspace::{EnvironmentFromNameError, get_activated_environment_variables},
};
use pixi_manifest::EnvironmentName;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(target_family = "unix")]
use pixi_pty::unix::PtySession;

use pixi_utils::prefix::Prefix;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ShellOptions {
    /// The environment to activate in the shell
    pub environment: Option<EnvironmentName>,

    pub lock_file_usage: LockFileUsage,

    pub no_install: bool,
}

/// An error that can occur while starting a shell in a workspace environment.
#[derive(Debug, Error, Diagnostic)]
pub enum ShellError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    EnvironmentFromName(#[from] EnvironmentFromNameError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    LockFileAndPrefixes(#[from] LockFileAndPrefixesError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Activation(#[from] ActivationError),

    #[error("unsupported shell '{0}'")]
    UnsupportedShell(String),

    #[error("failed to generate the activation script")]
    GenerateActivationScript(#[from] rattler_shell::shell::ShellError),

    #[error("failed to create the activation script")]
    CreateActivationScript(#[source] std::io::Error),

    #[error("failed to find '{executable}'")]
    FindShell {
        executable: String,
        #[source]
        source: which::Error,
    },

    #[error("failed to run '{executable}'")]
    RunShell {
        executable: String,
        #[source]
        source: std::io::Error,
    },
}

impl ShellError {
    fn run_shell(executable: &str) -> impl Fn(std::io::Error) -> Self + Copy + '_ {
        move |source| Self::RunShell {
            executable: executable.to_string(),
            source,
        }
    }
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
) -> Result<Option<i32>, ShellError> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .map_err(ShellError::CreateActivationScript)?;

    let mut shell_script =
        ShellScript::new(pwsh.clone(), Subdir::current().unwrap_or(Subdir::NoArch));
    for (key, value) in env {
        shell_script.set_env_var(key, value)?;
    }
    temp_file
        .write_all(shell_script.contents()?.as_bytes())
        .and_then(|_| temp_file.write_all(prompt.as_bytes()))
        .and_then(|_| temp_file.flush())
        .map_err(ShellError::CreateActivationScript)?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(pwsh.executable());
    command.arg("-NoLogo");
    command.arg("-NoExit");
    command.arg("-File");
    command.arg(&temp_path);

    ignore_ctrl_c();

    let run_shell = ShellError::run_shell(pwsh.executable());
    let mut process = command.spawn().map_err(run_shell)?;
    Ok(process.wait().map_err(run_shell)?.code())
}

// allowing dead code so that we test this on unix compilation as well
#[cfg_attr(unix, expect(unused))]
fn start_cmdexe(
    cmdexe: CmdExe,
    env: &HashMap<String, String>,
    prompt: String,
) -> Result<Option<i32>, ShellError> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .suffix(".cmd")
        .tempfile()
        .map_err(ShellError::CreateActivationScript)?;

    // TODO: Should we just execute the activation scripts directly for cmd.exe?
    let mut shell_script = ShellScript::new(cmdexe, Subdir::current().unwrap_or(Subdir::NoArch));
    for (key, value) in env {
        shell_script.set_env_var(key, value)?;
    }
    temp_file
        .write_all(shell_script.contents()?.as_bytes())
        .and_then(|_| temp_file.write_all(prompt.as_bytes()))
        .and_then(|_| temp_file.flush())
        .map_err(ShellError::CreateActivationScript)?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(cmdexe.executable());
    command.arg("/K");
    command.arg(&temp_path);

    ignore_ctrl_c();

    let run_shell = ShellError::run_shell(cmdexe.executable());
    let mut process = command.spawn().map_err(run_shell)?;
    Ok(process.wait().map_err(run_shell)?.code())
}

// allowing dead code so that we test this on unix compilation as well
#[cfg_attr(unix, expect(unused))]
fn start_winbash(
    bash: Bash,
    env: &HashMap<String, String>,
    prompt: String,
    prefix: &Prefix,
    source_shell_completions: bool,
) -> Result<Option<i32>, ShellError> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", bash.extension()))
        .rand_bytes(3)
        .tempfile()
        .map_err(ShellError::CreateActivationScript)?;

    let mut shell_script = ShellScript::new(bash, Subdir::current().unwrap_or(Subdir::NoArch));
    for (key, value) in env {
        if key == "PATH" || key == "Path" {
            // For Git Bash on Windows, the PATH must be formatted as POSIX paths according
            // to the cygpath command, and separated by ":" instead of ";". Use the
            // shell_script.set_path call to handle these details.
            let paths = value
                .split(";")
                .map(PathBuf::from)
                .collect::<Vec<PathBuf>>();
            shell_script.set_path(&paths, PathModificationBehavior::Replace)?;
        } else {
            shell_script.set_env_var(key, value)?;
        }
    }
    if source_shell_completions && let Some(completions_dir) = bash.completion_script_location() {
        shell_script.source_completions(&prefix.root().join(completions_dir))?;
    }
    temp_file
        .write_all(shell_script.contents()?.as_bytes())
        .and_then(|_| temp_file.write_all(prompt.as_bytes()))
        .and_then(|_| temp_file.flush())
        .map_err(ShellError::CreateActivationScript)?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let bash_path = which(bash.executable()).map_err(|source| ShellError::FindShell {
        executable: bash.executable().to_string(),
        source,
    })?;
    let mut command = std::process::Command::new(bash_path);
    command.arg("--init-file");
    command.arg(&temp_path);
    command.arg("-i");

    ignore_ctrl_c();

    let run_shell = ShellError::run_shell(bash.executable());
    let mut process = command.spawn().map_err(run_shell)?;
    Ok(process.wait().map_err(run_shell)?.code())
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
) -> Result<Option<i32>, ShellError> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", shell.extension()))
        .rand_bytes(3)
        .tempfile()
        .map_err(ShellError::CreateActivationScript)?;

    let mut shell_script = ShellScript::new(shell, Subdir::current().unwrap_or(Subdir::NoArch));
    for (key, value) in env {
        shell_script.set_env_var(key, value)?;
    }

    if source_shell_completions && let Some(completions_dir) = shell.completion_script_location() {
        shell_script.source_completions(&prefix.root().join(completions_dir))?;
    }
    const DONE_STR: &str = "PIXI_SHELL_ACTIVATION_DONE";
    shell_script
        .echo(DONE_STR)
        .map_err(rattler_shell::shell::ShellError::from)?;

    temp_file
        .write_all(shell_script.contents()?.as_bytes())
        .and_then(|_| temp_file.write_all(prompt.as_bytes()))
        .and_then(|_| temp_file.flush())
        .map_err(ShellError::CreateActivationScript)?;

    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(shell.executable());
    command.args(&args);

    // Space added before `source` to automatically ignore it in history.
    let mut source_command = " ".to_string();
    shell.run_script(&mut source_command, temp_path.as_ref())?;

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
    let run_shell = ShellError::run_shell(shell.executable());
    let mut process = PtySession::new(command).map_err(run_shell)?;
    process.send_line(source_command).map_err(run_shell)?;

    process.interact(Some(DONE_STR)).map_err(run_shell)
}

/// Starts a nu shell.
/// # Arguments
/// - `shell`: The Nushell (also contains executable location)
/// - `env`: A HashMap containing environment variables to set in the shell.
async fn start_nu_shell(
    shell: rattler_shell::shell::NuShell,
    env: &HashMap<String, String>,
    prompt: String,
) -> Result<Option<i32>, ShellError> {
    // create a tempfile for activation
    let mut temp_file = tempfile::Builder::new()
        .prefix("pixi_env_")
        .suffix(&format!(".{}", shell.extension()))
        .rand_bytes(3)
        .tempfile()
        .map_err(ShellError::CreateActivationScript)?;

    let mut shell_script = ShellScript::new(shell, Subdir::current().unwrap_or(Subdir::NoArch));
    for (key, value) in env {
        if key == "PATH" {
            let paths = std::env::split_paths(value).collect::<Vec<_>>();
            shell_script.set_path(&paths, PathModificationBehavior::Replace)?;
        } else {
            shell_script.set_env_var(key, value)?;
        }
    }

    temp_file
        .write_all(shell_script.contents()?.as_bytes())
        .and_then(|_| temp_file.write_all(prompt.as_bytes()))
        .and_then(|_| temp_file.flush())
        .map_err(ShellError::CreateActivationScript)?;

    let mut command = std::process::Command::new(shell.executable());
    command.arg("--execute");
    command.arg(format!("source {}", temp_file.path().display()));

    let run_shell = ShellError::run_shell(shell.executable());
    let mut process = command.spawn().map_err(run_shell)?;
    Ok(process.wait().map_err(run_shell)?.code())
}

pub async fn shell(
    workspace: &Workspace,
    options: ShellOptions,
    progress: Option<&Arc<pixi_reporters::TopLevelProgress>>,
) -> Result<i32, ShellError> {
    let environment = match options.environment {
        Some(name) => workspace
            .environment(&name)
            .ok_or(EnvironmentFromNameError::Unknown(name))?,
        None => workspace.default_environment(),
    };

    // Make sure environment is up-to-date, default to install, users can avoid this with frozen or locked.
    #[allow(unused_variables)]
    let (lock_file_data, prefix) = get_update_lock_file_and_prefix(
        &environment,
        progress.cloned(),
        UpdateMode::QuickValidate,
        UpdateLockFileOptions {
            lock_file_usage: options.lock_file_usage,
            no_install: options.no_install,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
            ..Default::default()
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
        &environment.activation_platform(),
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
            return Err(ShellError::UnsupportedShell(
                interactive_shell.executable().to_string(),
            ));
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
                return Err(ShellError::UnsupportedShell(
                    interactive_shell.executable().to_string(),
                ));
            }
        }
    };

    Ok(res?.unwrap_or(0))
}
