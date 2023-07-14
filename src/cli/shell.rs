use crate::environment::get_up_to_date_prefix;
use crate::project::environment::add_metadata_as_env_vars;
use crate::Project;
use clap::Parser;
use miette::IntoDiagnostic;
use rattler_conda_types::Platform;
use rattler_shell::activation::{ActivationVariables, Activator, PathModificationBehaviour};
use rattler_shell::shell::{Shell, ShellEnum};
use std::path::PathBuf;

/// Start a shell in the pixi environment of the project
#[derive(Parser, Debug)]
pub struct Args {
    /// The path to 'pixi.toml'
    #[arg(long)]
    manifest_path: Option<PathBuf>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())?;

    // Determine the current shell
    let shell: ShellEnum = ShellEnum::default();

    // Construct an activator so we can run commands from the environment
    let prefix = get_up_to_date_prefix(&project).await?;
    let activation_scripts: Vec<_> = project
        .activation_scripts(Platform::current())?
        .into_iter()
        .collect();
    let mut activator = Activator::from_path(prefix.root(), shell.clone(), Platform::current())
        .into_diagnostic()?;

    activator.activation_scripts.extend(activation_scripts);

    let activator_result = activator
        .activation(ActivationVariables {
            // Get the current PATH variable
            path: Default::default(),

            // Start from an empty prefix
            conda_prefix: None,

            // Prepending environment paths so they get found first.
            path_modification_behaviour: PathModificationBehaviour::Prepend,
        })
        .into_diagnostic()?;

    // Generate a temporary file with the script to execute. This includes the activation of the
    // environment.
    let mut script = format!("{}\n", activator_result.script.trim());

    // Add meta data env variables to help user interact with there configuration.
    add_metadata_as_env_vars(&mut script, &shell, &project)?;

    // Add the conda default env variable so that the tools that use this know it exists.
    shell
        .set_env_var(&mut script, "CONDA_DEFAULT_ENV", project.name())
        .into_diagnostic()?;

    // Start the shell as the last part of the activation script based on the default shell.
    let interactive_shell: ShellEnum = ShellEnum::from_parent_process()
        .or_else(ShellEnum::from_env)
        .unwrap_or_default();
    script.push_str(interactive_shell.executable());

    // Write the contents of the script to a temporary file that we can execute with the shell.
    let mut temp_file = tempfile::Builder::new()
        .suffix(&format!(".{}", shell.extension()))
        .tempfile()
        .into_diagnostic()?;
    std::io::Write::write_all(&mut temp_file, script.as_bytes()).into_diagnostic()?;

    // Execute the script with the shell
    let mut command = shell
        .create_run_script_command(temp_file.path())
        .spawn()
        .expect("failed to execute process");

    std::process::exit(command.wait().into_diagnostic()?.code().unwrap_or(1));
}
