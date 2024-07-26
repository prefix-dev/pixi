use std::{collections::HashMap, io::Write, path::PathBuf};

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::{
    activation::PathModificationBehavior,
    shell::{CmdExe, PowerShell, Shell, ShellEnum, ShellScript},
};

use crate::{
    activation::CurrentEnvVarBehavior, cli::LockFileUsageArgs, environment::get_up_to_date_prefix,
    project::virtual_packages::verify_current_platform_has_required_virtual_packages, prompt,
    Project,
};
use pixi_config::ConfigCliPrompt;
use pixi_manifest::EnvironmentName;
#[cfg(target_family = "unix")]
use pixi_pty::unix::PtySession;

/// Start a shell in the pixi environment of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    #[clap(flatten)]
    lock_file_usage: LockFileUsageArgs,

    /// The environment to activate in the shell
    #[arg(long, short)]
    environment: Option<String>,

    #[clap(flatten)]
    config: ConfigCliPrompt,
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
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    // close the file handle, but keep the path (needed for Windows)
    let temp_path = temp_file.into_temp_path();

    let mut command = std::process::Command::new(pwsh.executable());
    command.arg("-NoLogo");
    command.arg("-NoExit");
    command.arg("-File");
    command.arg(&temp_path);

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
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(cmdexe.executable());
    command.arg("/K");
    command.arg(temp_file.path());

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

    temp_file
        .write_all(shell_script.contents().into_diagnostic()?.as_bytes())
        .into_diagnostic()?;

    // Write custom prompt to the env file
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.args(args);

    // Space added before `source` to automatically ignore it in history.
    let mut source_command = " ".to_string();
    shell
        .run_script(&mut source_command, temp_file.path())
        .into_diagnostic()?;

    // Remove automatically added `\n`, if for some reason this fails, just ignore.
    let source_command = source_command
        .strip_suffix('\n')
        .unwrap_or(source_command.as_str());

    // Start process and send env activation to the shell.
    let mut process = PtySession::new(command).into_diagnostic()?;
    process.send_line(source_command).into_diagnostic()?;

    process.interact().into_diagnostic()
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
    temp_file.write(prompt.as_bytes()).into_diagnostic()?;

    let mut command = std::process::Command::new(shell.executable());
    command.arg("--execute");
    command.arg(format!("source {}", temp_file.path().display()));

    let mut process = command.spawn().into_diagnostic()?;
    Ok(process.wait().into_diagnostic()?.code())
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project =
        Project::load_or_else_discover(args.manifest_path.as_deref())?.with_cli_config(args.config);
    let environment = project.environment_from_name_or_env_var(args.environment)?;

    verify_current_platform_has_required_virtual_packages(&environment).into_diagnostic()?;

    let prompt_name = match environment.name() {
        EnvironmentName::Default => project.name().to_string(),
        EnvironmentName::Named(name) => format!("{}:{}", project.name(), name),
    };

    // Make sure environment is up-to-date, default to install, users can avoid this with frozen or locked.
    get_up_to_date_prefix(&environment, args.lock_file_usage.into(), false).await?;

    // Get the environment variables we need to set activate the environment in the shell.
    let env = project
        .get_activated_environment_variables(&environment, CurrentEnvVarBehavior::Exclude)
        .await?;

    tracing::debug!("Pixi environment activation:\n{:?}", env);

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();

    let prompt = if project.config().change_ps1() {
        match interactive_shell {
            ShellEnum::NuShell(_) => prompt::get_nu_prompt(prompt_name.as_str()),
            ShellEnum::PowerShell(_) => prompt::get_powershell_prompt(prompt_name.as_str()),
            ShellEnum::Bash(_) => prompt::get_bash_hook(prompt_name.as_str()),
            ShellEnum::Zsh(_) => prompt::get_zsh_hook(prompt_name.as_str()),
            ShellEnum::Fish(_) => prompt::get_fish_prompt(prompt_name.as_str()),
            ShellEnum::Xonsh(_) => prompt::get_xonsh_prompt(),
            ShellEnum::CmdExe(_) => prompt::get_cmd_prompt(prompt_name.as_str()),
        }
    } else {
        "".to_string()
    };

    #[cfg(target_family = "windows")]
    let res = match interactive_shell {
        ShellEnum::NuShell(nushell) => start_nu_shell(nushell, env, prompt).await,
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, env, prompt),
        ShellEnum::CmdExe(cmdexe) => start_cmdexe(cmdexe, env, prompt),
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell);
        }
    };

    #[cfg(target_family = "unix")]
    let res = match interactive_shell {
        ShellEnum::NuShell(nushell) => start_nu_shell(nushell, env, prompt).await,
        ShellEnum::PowerShell(pwsh) => start_powershell(pwsh, env, prompt),
        ShellEnum::Bash(bash) => start_unix_shell(bash, vec!["-l", "-i"], env, prompt).await,
        ShellEnum::Zsh(zsh) => start_unix_shell(zsh, vec!["-l", "-i"], env, prompt).await,
        ShellEnum::Fish(fish) => start_unix_shell(fish, vec![], env, prompt).await,
        ShellEnum::Xonsh(xonsh) => start_unix_shell(xonsh, vec![], env, prompt).await,
        _ => {
            miette::bail!("Unsupported shell: {:?}", interactive_shell)
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
