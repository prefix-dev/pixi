use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsString,
    fmt::{Display, Formatter},
    path::PathBuf,
};

use deno_task_shell::{
    ShellPipeWriter, ShellState, execute_with_pipes, parser::SequentialList, pipe,
};
use fs_err::tokio as tokio_fs;
use itertools::Itertools;
use miette::{Context, Diagnostic};
use pixi_consts::consts;
use pixi_core::{
    Workspace,
    activation::CurrentEnvVarBehavior,
    workspace::get_activated_environment_variables,
    workspace::{Environment, HasWorkspaceRef},
};
use pixi_manifest::{Task, TaskName, task::ArgValues, task::TemplateStringError};
use pixi_progress::await_in_progress;
use rattler_lock::LockFile;
use thiserror::Error;
use tokio::task::JoinHandle;

use crate::task_graph::{TaskGraph, TaskId};
use crate::task_hash::{InputHashesError, NameHash, TaskCache, TaskHash};

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
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|name| name.as_str())
    }

    /// Returns the task description from the project.
    pub fn task(&self) -> &Task {
        self.task.as_ref()
    }

    /// Returns the project in which this task is defined.
    pub fn project(&self) -> &'p Workspace {
        self.workspace
    }

    pub fn args(&self) -> &ArgValues {
        &self.args
    }

    /// Returns the task as script
    fn as_script(&self) -> Result<Option<String>, FailedToParseShellScript> {
        // Convert the task into an executable string
        let task = self
            .task
            .as_single_command(Some(&self.args))
            .map_err(FailedToParseShellScript::ArgumentReplacement)?;
        if let Some(task) = task {
            // Get the export specific environment variables
            let export = get_export_specific_task_env(self.task.as_ref());

            // Append the command line arguments verbatim
            let cli_args = if let ArgValues::FreeFormArgs(additional_args) = &self.args {
                additional_args
                    .iter()
                    .format_with(" ", |arg, f| f(&format_args!("'{}'", arg)))
                    .to_string()
            } else {
                String::new()
            };

            // Skip the export if it's empty, to avoid newlines
            let full_script = if export.is_empty() {
                format!("{} {}", task, cli_args)
            } else {
                format!("{}\n{} {}", export, task, cli_args)
            };

            Ok(Some(full_script))
        } else {
            Ok(None)
        }
    }

    /// Returns a [`SequentialList`] which can be executed by deno task shell.
    /// Returns `None` if the command is not executable like in the case of
    /// an alias.
    pub fn as_deno_script(&self) -> Result<Option<SequentialList>, FailedToParseShellScript> {
        let full_script = self.as_script()?;

        if let Some(full_script) = full_script {
            tracing::debug!("Parsing shell script: {}", full_script);

            // Parse the shell command
            deno_task_shell::parser::parse(full_script.trim())
                .map_err(|e| FailedToParseShellScript::ParseError {
                    source: e,
                    task: full_script.to_string(),
                })
                .map(Some)
        } else {
            Ok(None)
        }
    }

    /// Returns the working directory for this task.
    pub fn working_directory(&self) -> Result<PathBuf, InvalidWorkingDirectory> {
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
    pub fn full_command(&self) -> Result<Option<String>, TemplateStringError> {
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
    pub fn display_command(&self) -> impl Display + '_ {
        ExecutableTaskConsoleDisplay { task: self }
    }

    /// Executes the task and capture its output.
    pub async fn execute_with_pipes(
        &self,
        command_env: &HashMap<OsString, OsString>,
        input: Option<&[u8]>,
    ) -> Result<RunOutput, TaskExecutionError> {
        let Some(script) = self.as_deno_script()? else {
            return Ok(RunOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
        };
        let cwd = self.working_directory()?;
        let (stdin, mut stdin_writer) = pipe();
        if let Some(stdin) = input {
            stdin_writer
                .write_all(stdin)
                .expect("should be able to write to stdin");
        }
        drop(stdin_writer); // prevent a deadlock by dropping the writer
        let (stdout, stdout_handle) = get_output_writer_and_handle();
        let (stderr, stderr_handle) = get_output_writer_and_handle();
        let state = ShellState::new(
            command_env.clone(),
            cwd,
            Default::default(),
            Default::default(),
        );
        let code = execute_with_pipes(script, state, stdin, stdout, stderr).await;
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
    pub async fn can_skip(&self, lock_file: &LockFile) -> Result<CanSkip, std::io::Error> {
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
    pub async fn save_cache(
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
