use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsString,
    fmt::{Display, Formatter},
    io::Write,
    path::PathBuf,
};

use deno_task_shell::{
    ShellPipeWriter, ShellState, execute_with_pipes, parser::SequentialList, pipe,
};
use fs_err::tokio as tokio_fs;

/// Contains the prepared execution data for a task with interpreter
pub(crate) struct PreparedExecution {
    pub script: SequentialList,
    pub stdin: deno_task_shell::ShellPipeReader,
}
use itertools::Itertools;
use miette::{Context, Diagnostic};
use pixi_consts::consts;
use pixi_manifest::{Task, TaskName, task::ArgValues, task::TemplateStringError};
use pixi_progress::await_in_progress;
use rattler_lock::LockFile;
use thiserror::Error;
use tokio::task::JoinHandle;

use super::task_hash::{InputHashesError, NameHash, TaskCache, TaskHash};
use crate::{
    Workspace,
    activation::CurrentEnvVarBehavior,
    task::task_graph::{TaskGraph, TaskId},
    workspace::get_activated_environment_variables,
    workspace::{Environment, HasWorkspaceRef},
};

/// Runs task in project.
#[derive(Default, Debug)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error, Diagnostic)]
#[error("The task failed to parse")]
pub enum FailedToParseShellScript {
    #[error("failed to parse shell script. Task: '{task}'")]
    ParseError {
        #[source]
        source: anyhow::Error,
        task: String,
    },

    #[error(transparent)]
    #[diagnostic(transparent)]
    ArgumentReplacement(#[from] TemplateStringError),
}

#[derive(Debug, Error, Diagnostic)]
#[error("invalid working directory '{path}'")]
pub struct InvalidWorkingDirectory {
    pub path: String,
}

#[derive(Debug, Error, Diagnostic)]
pub enum TaskExecutionError {
    #[error(transparent)]
    InvalidWorkingDirectory(#[from] InvalidWorkingDirectory),

    #[error(transparent)]
    FailedToParseShellScript(#[from] FailedToParseShellScript),
}

#[derive(Debug, Error, Diagnostic)]
pub enum CacheUpdateError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TaskHashError(#[from] InputHashesError),

    #[error("failed to serialize cache")]
    Serialization(#[from] serde_json::Error),
}

pub enum CanSkip {
    Yes,
    No(Option<TaskHash>),
}

/// A task that contains enough information to be able to execute it. The
/// lifetime [`'p`] refers to the lifetime of the project that contains the
/// tasks.
#[derive(Clone, Debug)]
pub struct ExecutableTask<'p> {
    pub workspace: &'p Workspace,
    pub name: Option<TaskName>,
    pub task: Cow<'p, Task>,
    pub run_environment: Environment<'p>,
    pub args: ArgValues,
}

impl<'p> ExecutableTask<'p> {
    /// Constructs a new executable task from a task graph node.
    pub fn from_task_graph(task_graph: &TaskGraph<'p>, task_id: TaskId) -> Self {
        let node = &task_graph[task_id];

        Self {
            workspace: task_graph.project(),
            name: node.name.clone(),
            task: node.task.clone(),
            run_environment: node.run_environment.clone(),
            args: node.args.clone().unwrap_or_default(),
        }
    }

    /// Returns the name of the task or `None` if this is an anonymous task.
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|name| name.as_str())
    }

    /// Returns the task description from the project.
    pub(crate) fn task(&self) -> &Task {
        self.task.as_ref()
    }

    /// Returns the project in which this task is defined.
    pub(crate) fn project(&self) -> &'p Workspace {
        self.workspace
    }

    pub(crate) fn args(&self) -> &ArgValues {
        &self.args
    }

    /// Returns the task as script
    fn as_script(&self) -> Result<Option<String>, FailedToParseShellScript> {
        // Convert the task into an executable string (with args but without freeargs)
        let task = self
            .task
            .as_single_command(Some(&self.args))
            .map_err(FailedToParseShellScript::ArgumentReplacement)?;

        let Some(task) = task else {
            return Ok(None);
        };

        // Get export environment variables
        let export = get_export_specific_task_env(self.task.as_ref());

        // Get freeargs for both modes
        let freeargs = if let ArgValues::FreeFormArgs(additional_args) = &self.args {
            if additional_args.is_empty() {
                String::new()
            } else {
                format!(" {}", additional_args
                    .iter()
                    .format_with(" ", |arg, f| f(&format_args!("'{}'", arg))))
            }
        } else {
            String::new()
        };

        if let Some(interpreter) = self.task().interpreter() {
            // Interpreter mode: create temp file and build interpreter command
            let script_content = task.into_owned();

            // Create a temporary file to store the script
            let mut temp_file =
                tempfile::NamedTempFile::new().expect("Failed to create temporary file");
            temp_file
                .write_all(script_content.as_bytes())
                .expect("Failed to write script to temporary file");
            temp_file.flush().expect("Failed to flush temporary file");

            // Get the temporary file path
            let temp_path = temp_file.path().to_string_lossy().to_string();

            // Handle {0} placeholder for interpreter command
            let interpreter_with_file = if interpreter.contains("{0}") {
                // Replace {0} placeholder with the temporary file path
                interpreter.replace("{0}", &temp_path)
            } else {
                // Default behavior: append the temporary file path at the end
                format!("{interpreter} {temp_path}")
            };

            // Build final command: {export} {interpreter_with_file} {freeargs}
            let final_command = if export.is_empty() {
                format!("{interpreter_with_file}{freeargs}")
            } else {
                format!("{export}\n{interpreter_with_file}{freeargs}")
            };

            // Keep temp file alive by storing it in a static location
            // The file will be cleaned up when the process exits
            let _ = temp_file.keep().expect("Failed to persist temporary file");

            Ok(Some(final_command))
        } else {
            // Cmd mode: build command with freeargs
            let cmd_with_args = task.into_owned();

            // Build final command: {export} {cmd_with_args} {freeargs}
            let final_command = if export.is_empty() {
                format!("{cmd_with_args}{freeargs}")
            } else {
                format!("{export}\n{cmd_with_args}{freeargs}")
            };

            Ok(Some(final_command))
        }
    }

    /// Returns a [`SequentialList`] which can be executed by deno task shell.
    /// Returns `None` if the command is not executable like in the case of
    /// an alias.
    ///
    /// This is used for cmd mode execution only (when no interpreter is specified).
    pub(crate) fn as_deno_script(
        &self,
    ) -> Result<Option<SequentialList>, FailedToParseShellScript> {
        // Only for cmd mode (no interpreter)
        if self.task().interpreter().is_some() {
            return Ok(None);
        }

        let Some(full_script) = self.as_script()? else {
            return Ok(None);
        };

        tracing::debug!("Parsing shell script: {}", full_script);

        // Parse the shell command (export and freeargs are already included in as_script)
        deno_task_shell::parser::parse(full_script.trim())
            .map_err(|e| FailedToParseShellScript::ParseError {
                source: e,
                task: full_script.to_string(),
            })
            .map(Some)
    }

    /// Returns the working directory for this task.
    pub(crate) fn working_directory(&self) -> Result<PathBuf, InvalidWorkingDirectory> {
        Ok(match self.task.working_directory() {
            Some(cwd) if cwd.is_absolute() => cwd.to_path_buf(),
            Some(cwd) => {
                let abs_path = self.workspace.root().join(cwd);
                if !abs_path.is_dir() {
                    return Err(InvalidWorkingDirectory {
                        path: cwd.to_string_lossy().to_string(),
                    });
                }
                abs_path
            }
            None => self.workspace.root().to_path_buf(),
        })
    }

    /// Returns the full command that should be executed for this task. This
    /// includes any additional arguments that should be passed to the
    /// command.
    ///
    /// This function returns `None` if the task does not define a command to
    /// execute. This is the case for alias only commands.
    pub(crate) fn full_command(&self) -> Result<Option<String>, TemplateStringError> {
        let original_cmd = self
            .task
            .as_single_command(Some(&self.args))?
            .map(|c| c.into_owned());

        if let Some(mut cmd) = original_cmd {
            if let ArgValues::FreeFormArgs(additional_args) = &self.args {
                if !additional_args.is_empty() {
                    cmd.push(' ');
                    cmd.push_str(&additional_args.join(" "));
                }
            }
            Ok(Some(cmd))
        } else {
            Ok(None)
        }
    }

    /// Returns an object that implements [`Display`] which outputs the command
    /// of the wrapped task.
    pub(crate) fn display_command(&self) -> impl Display + '_ {
        ExecutableTaskConsoleDisplay { task: self }
    }

    /// Prepares the script and stdin pipe for execution.
    ///
    /// This method handles the common logic for both `execute_with_pipes` and the CLI's `execute_task`.
    /// Returns None if there is no script to execute (e.g., for alias tasks).
    pub(crate) fn prepare_execution(
        &self,
    ) -> Result<Option<PreparedExecution>, FailedToParseShellScript> {
        if self.task().interpreter().is_some() {
            // Interpreter mode: use as_script to get the full command
            let Some(final_command) = self.as_script()? else {
                return Ok(None);
            };

            tracing::debug!("Parsing interpreter command: {}", final_command);

            // Parse the interpreter command directly
            let parsed_script = deno_task_shell::parser::parse(final_command.trim())
                .map_err(|e| FailedToParseShellScript::ParseError {
                    source: e,
                    task: final_command.to_string(),
                })?;

            let stdin = deno_task_shell::ShellPipeReader::stdin();

            Ok(Some(PreparedExecution {
                script: parsed_script,
                stdin,
            }))
        } else {
            // Cmd mode: use as_deno_script for proper deno shell parsing
            let Some(deno_script) = self.as_deno_script()? else {
                return Ok(None);
            };

            let stdin = deno_task_shell::ShellPipeReader::stdin();
            Ok(Some(PreparedExecution {
                script: deno_script,
                stdin,
            }))
        }
    }

    /// Executes the task and capture its output.
    pub async fn execute_with_pipes(
        &self,
        command_env: &HashMap<OsString, OsString>,
    ) -> Result<RunOutput, TaskExecutionError> {
        let Some(prepared) = self.prepare_execution()? else {
            // No script to execute, return empty output
            return Ok(RunOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
        };

        let cwd = self.working_directory()?;
        let (stdout, stdout_handle) = get_output_writer_and_handle();
        let (stderr, stderr_handle) = get_output_writer_and_handle();
        let state = ShellState::new(
            command_env.clone(),
            cwd,
            Default::default(),
            Default::default(),
        );
        let code = execute_with_pipes(prepared.script, state, prepared.stdin, stdout, stderr).await;
        Ok(RunOutput {
            exit_code: code,
            stdout: stdout_handle.await.expect("should be able to get stdout"),
            stderr: stderr_handle.await.expect("should be able to get stderr"),
        })
    }

    /// We store the hashes of the inputs and the outputs of the task in a file
    /// in the cache. The current name is something like
    /// `run_environment-task_name.json`.
    pub(crate) fn cache_name(&self, args_cache: Option<NameHash>) -> String {
        format!(
            "{}-{}-{}.json",
            self.run_environment.name(),
            self.name().unwrap_or("default"),
            args_cache
                .map(|hash| hash.to_string())
                .unwrap_or("".to_string())
        )
    }

    /// Checks if the task can be skipped. If the task can be skipped, it
    /// returns `CanSkip::Yes`. If the task cannot be skipped, it returns
    /// `CanSkip::No` and includes the hash of the task that caused the task
    /// to not be skipped - we can use this later to update the cache file
    /// quickly.
    pub(crate) async fn can_skip(&self, lock_file: &LockFile) -> Result<CanSkip, std::io::Error> {
        tracing::info!("Checking if task can be skipped");
        let args_hash = TaskHash::task_args_hash(self).unwrap_or_default();
        let cache_name = self.cache_name(args_hash);
        let cache_file = self.project().task_cache_folder().join(cache_name);
        if cache_file.exists() {
            let cache = tokio_fs::read_to_string(&cache_file).await?;
            let cache: TaskCache = serde_json::from_str(&cache)?;
            let hash = TaskHash::from_task(self, lock_file).await;
            if let Ok(Some(hash)) = hash {
                if hash.computation_hash() != cache.hash {
                    return Ok(CanSkip::No(Some(hash)));
                } else {
                    return Ok(CanSkip::Yes);
                }
            }
        }
        Ok(CanSkip::No(None))
    }

    /// Saves the cache of the task. This function will update the cache file
    /// with the new hash of the task (inputs and outputs). If the task has
    /// no hash, it will not save the cache.
    pub(crate) async fn save_cache(
        &self,
        lock_file: &LockFile,
        previous_hash: Option<TaskHash>,
    ) -> Result<(), CacheUpdateError> {
        let task_cache_folder = self.project().task_cache_folder();
        let args_cache = TaskHash::task_args_hash(self)?;
        let cache_file = task_cache_folder.join(self.cache_name(args_cache));
        let new_hash = if let Some(mut previous_hash) = previous_hash {
            previous_hash.update_output(self).await?;
            previous_hash
        } else if let Some(hash) = TaskHash::from_task(self, lock_file).await? {
            hash
        } else {
            return Ok(());
        };

        tokio::fs::create_dir_all(&task_cache_folder).await?;

        let cache = TaskCache {
            hash: new_hash.computation_hash(),
        };
        let cache = serde_json::to_string(&cache)?;
        Ok(tokio::fs::write(&cache_file, cache).await?)
    }
}

/// A helper object that implements [`Display`] to display (with ascii color)
/// the command of the task.
struct ExecutableTaskConsoleDisplay<'p, 't> {
    task: &'t ExecutableTask<'p>,
}

impl Display for ExecutableTaskConsoleDisplay<'_, '_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.task.task.as_single_command(Some(&self.task.args)) {
            Ok(command) => {
                write!(
                    f,
                    "{}",
                    consts::TASK_STYLE
                        .apply_to(command.as_deref().unwrap_or("<alias>"))
                        .bold()
                )?;
                if let ArgValues::FreeFormArgs(additional_args) = &self.task.args {
                    if !additional_args.is_empty() {
                        write!(
                            f,
                            " {}",
                            consts::TASK_STYLE.apply_to(additional_args.iter().format(" "))
                        )?;
                    }
                }
                Ok(())
            }
            Err(err) => {
                write!(
                    f,
                    "{}",
                    consts::TASK_ERROR_STYLE.apply_to(err.get_source()).bold()
                )
            }
        }
    }
}
/// Helper function to create a pipe that we can get the output from.
fn get_output_writer_and_handle() -> (ShellPipeWriter, JoinHandle<String>) {
    let (reader, writer) = pipe();
    let handle = reader.pipe_to_string_handle();
    (writer, handle)
}

/// Task specific environment variables.
fn get_export_specific_task_env(task: &Task) -> String {
    // Append the environment variables if they don't exist
    let mut export = String::new();
    if let Some(env) = task.env() {
        for (key, value) in env {
            if value.contains(format!("${}", key).as_str()) || std::env::var(key.as_str()).is_err()
            {
                tracing::info!("Setting environment variable: {}=\"{}\"", key, value);
                export.push_str(&format!("export \"{}={}\";\n", key, value));
            } else {
                tracing::info!("Environment variable {} already set", key);
            }
        }
    }
    export
}

/// Determine the environment variables to use when executing a command. The
/// method combines the activation environment with the system environment
/// variables.
pub async fn get_task_env(
    environment: &Environment<'_>,
    clean_env: bool,
    lock_file: Option<&LockFile>,
    force_activate: bool,
    experimental_cache: bool,
) -> miette::Result<HashMap<String, String>> {
    // Get environment variables from the activation
    let env_var_behavior = if clean_env {
        CurrentEnvVarBehavior::Clean
    } else {
        CurrentEnvVarBehavior::Include
    };
    let mut activation_env = await_in_progress("activating environment", |_| {
        get_activated_environment_variables(
            environment.workspace().env_vars(),
            environment,
            env_var_behavior,
            lock_file,
            force_activate,
            experimental_cache,
        )
    })
    .await
    .wrap_err("failed to activate environment")?
    .clone();

    // Add the current working directory to the environment
    if let Ok(init_cwd) = std::env::current_dir() {
        activation_env.insert(
            "INIT_CWD".to_string(),
            init_cwd.to_string_lossy().to_string(),
        );
    } else {
        tracing::warn!("Failed to get the current working directory for INIT_CWD.");
    }

    // Concatenate with the system environment variables
    Ok(activation_env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const PROJECT_BOILERPLATE: &str = r#"
        [project]
        name = "foo"
        version = "0.1.0"
        channels = []
        # Required to run tests
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64", "linux-ppc64le", "linux-aarch64"]
        "#;

    #[test]
    fn test_export_specific_task_env() {
        let file_contents = r#"
            [tasks]
            test = {cmd = "test", cwd = "tests", env = {FOO = "bar", BAR = "$FOO"}}
            "#;
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            &format!("{PROJECT_BOILERPLATE}\n{file_contents}"),
        )
        .unwrap();

        let task = workspace
            .default_environment()
            .task(&TaskName::from("test"), None)
            .unwrap();

        let export = get_export_specific_task_env(task);

        assert_eq!(export, "export \"FOO=bar\";\nexport \"BAR=$FOO\";\n");
    }

    #[test]
    fn test_as_script() {
        let file_contents = r#"
            [tasks]
            test = {cmd = "test", cwd = "tests", env = {FOO = "bar"}}
            "#;

        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            &format!("{PROJECT_BOILERPLATE}\n{file_contents}"),
        )
        .unwrap();

        let task = workspace
            .default_environment()
            .task(&TaskName::from("test"), None)
            .unwrap();

        let executable_task = ExecutableTask {
            workspace: &workspace,
            name: Some("test".into()),
            task: Cow::Borrowed(task),
            run_environment: workspace.default_environment(),
            args: ArgValues::default(),
        };

        let script = executable_task.as_script().unwrap().unwrap();
        assert_eq!(script, "export \"FOO=bar\";\n\ntest ");
    }

    #[tokio::test]
    async fn test_get_task_env() {
        let file_contents = r#"
            [tasks]
            test = {cmd = "test", cwd = "tests", env = {FOO = "bar"}}
            "#;
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            &format!("{PROJECT_BOILERPLATE}\n{file_contents}"),
        )
        .unwrap();

        let environment = workspace.default_environment();
        let env = get_task_env(&environment, false, None, false, false)
            .await
            .unwrap();
        assert_eq!(
            env.get("INIT_CWD").unwrap(),
            &std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string()
        );
    }
}
