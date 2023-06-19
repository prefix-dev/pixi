use crate::environment::get_up_to_date_prefix;
use crate::Project;
use clap::Parser;
use rattler_conda_types::Platform;
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell::{Shell, ShellEnum};

#[derive(Parser, Debug)]
pub struct Args {}

pub async fn execute(_args: Args) -> anyhow::Result<()> {
    let project = Project::discover()?;

    // Determine the current shell
    let shell: ShellEnum = ShellEnum::detect_from_environment()
        .ok_or_else(|| anyhow::anyhow!("could not detect the current shell"))?;

    // Construct an activator so we can run commands from the environment
    let prefix = get_up_to_date_prefix(&project).await?;
    let activator = Activator::from_path(prefix.root(), shell.clone(), Platform::current())?;

    let activator_result = activator.activation(ActivationVariables {
        // Get the current PATH variable
        path: std::env::var_os("PATH").map(|path_var| std::env::split_paths(&path_var).collect()),

        // Start from an empty prefix
        conda_prefix: None,
    })?;

    // Generate a temporary file with the script to execute. This includes the activation of the
    // environment.
    let mut script = format!("{}\n", activator_result.script.trim());

    shell.set_env_var(&mut script, "CONDA_DEFAULT_ENV", project.name())?;

    script.push_str("$SHELL");

    // Write the contents of the script to a temporary file that we can execute with the shell.
    let mut temp_file = tempfile::Builder::new()
        .suffix(&format!(".{}", shell.extension()))
        .tempfile()?;
    std::io::Write::write_all(&mut temp_file, script.as_bytes())?;

    // Execute the script with the shell
    let mut command = shell
        .create_run_script_command(temp_file.path())
        .spawn()
        .expect("failed to execute process");

    std::process::exit(command.wait()?.code().unwrap_or(1));
}
