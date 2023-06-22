use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::{fmt::Write, path::PathBuf};

use crate::Project;
use clap::Parser;
use is_executable::IsExecutable;
use rattler_conda_types::Platform;

use crate::command::{CmdArgs, Command, ProcessCmd};
use crate::environment::get_up_to_date_prefix;
use rattler_shell::activation::ActivationResult;
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::{Shell, ShellEnum},
};

/// Runs command in project.
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// The command you want to run in the projects environment.
    command: Vec<String>,

    /// The path to the pixi project
    #[arg(long)]
    project_path: Option<PathBuf>,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let project = Project::discover(args.project_path)?;

    // Get the script to execute from the command line.
    let (command_name, command) = args
        .command
        .first()
        .and_then(|cmd_name| {
            project
                .command_opt(cmd_name)
                .map(|cmd| (Some(cmd_name.clone()), cmd.clone()))
        })
        .unwrap_or_else(|| {
            (
                None,
                Command::Process(ProcessCmd {
                    cmd: CmdArgs::Multiple(args.command),
                    depends_on: vec![],
                }),
            )
        });

    // Determine the current shell
    let shell: ShellEnum = ShellEnum::default();

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

    // Add meta data env variables to help user interact with there configuration.
    add_metadata_as_env_vars(&mut script, &shell, &project)?;

    // Perform post order traversal of the commands and their `depends_on` to make sure they are
    // executed in the right order.
    let mut s1 = VecDeque::new();
    let mut s2 = VecDeque::new();
    let mut added = HashSet::new();

    // Add the command specified on the command line first
    s1.push_back(command);
    if let Some(command_name) = command_name {
        added.insert(command_name);
    }

    while let Some(command) = s1.pop_back() {
        // Get the dependencies of the command
        let depends_on = match &command {
            Command::Process(process) => process.depends_on.as_slice(),
            Command::Alias(alias) => &alias.depends_on,
            _ => &[],
        };

        // Locate the dependencies in the project and add them to the stack
        for dependency in depends_on.iter() {
            if !added.contains(dependency) {
                let cmd = project
                    .command_opt(dependency)
                    .ok_or_else(|| anyhow::anyhow!("failed to find dependency {}", dependency))?
                    .clone();

                s1.push_back(cmd);
                added.insert(dependency.clone());
            }
        }

        s2.push_back(command)
    }

    while let Some(command) = s2.pop_back() {
        // Write the invocation of the command into the script.
        command.write_invoke_script(&mut script, &shell, &project, &activator_result)?;
    }

    tracing::debug!("Activation script:\n{}", script);

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

/// Given a command and arguments to invoke it, format it so that it is as generalized as possible.
///
/// The executable is also canonicalized. This means the executable path is looked up. If the
/// executable is not found either in the environment or in the project root an error is returned.
fn format_execute_command(
    project: &Project,
    path: &[PathBuf],
    args: &[String],
) -> anyhow::Result<Vec<String>> {
    // Determine the command location
    let command = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let command_path = find_command(command, project.root(), path.iter().map(|p| p.as_path()))
        .ok_or_else(|| anyhow::anyhow!("could not find executable '{command}'"))?;

    // Format all the commands and quote them properly.
    Ok([command_path.to_string_lossy().as_ref()]
        .into_iter()
        .chain(args.iter().skip(1).map(|x| x.as_ref()))
        .map(|arg| shlex::quote(arg).into_owned())
        .collect())
}

// Locate the specified command name in the project or environment
fn find_command<'a>(
    executable_name: &str,
    project_root: &'a Path,
    prefix_paths: impl IntoIterator<Item = &'a Path>,
) -> Option<PathBuf> {
    let executable_path = Path::new(executable_name);

    // Iterate over all search paths
    for search_path in [project_root].into_iter().chain(prefix_paths) {
        let absolute_executable_path = search_path.join(executable_path);

        // Try to locate an executable at this location
        if let Some(executable_path) = find_canonical_executable_path(&absolute_executable_path) {
            return Some(executable_path);
        }
    }

    None
}

// Given a relative executable path, try to find the canonical path
fn find_canonical_executable_path(path: &Path) -> Option<PathBuf> {
    // If the path already points to an existing executable there is nothing to do.
    match dunce::canonicalize(path) {
        Ok(path) if path.is_executable() => return Some(path),
        _ => {}
    }

    // Get executable extensions and see if by adding the extension we can turn it into a valid
    // path.
    for ext in executable_extensions() {
        let with_ext = path.with_extension(ext);
        match dunce::canonicalize(with_ext) {
            Ok(path) if path.is_executable() => return Some(path),
            _ => {}
        }
    }

    None
}

// Add pixi meta data into the environment as environment variables.
fn add_metadata_as_env_vars(
    script: &mut impl Write,
    shell: &ShellEnum,
    project: &Project,
) -> anyhow::Result<()> {
    // Setting a base prefix for the pixi package
    const PREFIX: &str = "PIXI_PACKAGE_";

    shell.set_env_var(
        script,
        &format!("{PREFIX}ROOT"),
        &(project.root().to_string_lossy()),
    )?;
    shell.set_env_var(
        script,
        &format!("{PREFIX}MANIFEST"),
        &(project.manifest_path().to_string_lossy()),
    )?;
    shell.set_env_var(
        script,
        &format!("{PREFIX}PLATFORMS"),
        &(project
            .platforms()
            .iter()
            .map(|plat| plat.as_str())
            .collect::<Vec<&str>>()
            .join(",")),
    )?;
    shell.set_env_var(script, &format!("{PREFIX}NAME"), project.name())?;
    shell.set_env_var(
        script,
        &format!("{PREFIX}VERSION"),
        &project.version().to_string(),
    )?;

    Ok(())
}

/// Returns all file extensions that are considered for executable files.
#[cfg(windows)]
fn executable_extensions() -> &'static [String] {
    use once_cell::sync::Lazy;
    static PATHEXT: Lazy<Vec<String>> = Lazy::new(|| {
        if let Some(pathext) = std::env::var_os("PATHEXT") {
            pathext
                .to_string_lossy()
                .split(';')
                // Filter out empty tokens and ';' at the end
                .filter(|f| f.len() > 1)
                // Cut off the leading '.' character
                .map(|ext| ext[1..].to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    });
    PATHEXT.as_slice()
}

/// Returns all file extensions that are considered for executable files.
#[cfg(not(windows))]
fn executable_extensions() -> &'static [String] {
    &[]
}

impl Command {
    /// Write the invocation of this command to the specified script.
    pub fn write_invoke_script(
        &self,
        contents: &mut String,
        shell: &ShellEnum,
        project: &Project,
        activation_result: &ActivationResult,
    ) -> anyhow::Result<()> {
        let args = match self {
            Command::Plain(cmd) => {
                let args = shlex::split(cmd)
                    .ok_or_else(|| anyhow::anyhow!("invalid quoted command arguments"))?;
                Some(format_execute_command(
                    project,
                    &activation_result.path,
                    &args,
                )?)
            }
            Command::Process(cmd) => {
                let args = match &cmd.cmd {
                    CmdArgs::Single(str) => shlex::split(str)
                        .ok_or_else(|| anyhow::anyhow!("invalid quoted command arguments"))?,
                    CmdArgs::Multiple(args) => args.to_vec(),
                };
                Some(format_execute_command(
                    project,
                    &activation_result.path,
                    &args,
                )?)
            }
            _ => None,
        };

        // If we have a command to execute, add it to the script.
        if let Some(args) = args {
            shell
                .run_command(contents, args.iter().map(|arg| arg.as_ref()))
                .expect("failed to write script");
            writeln!(contents).expect("failed to write script");
        }

        Ok(())
    }
}
