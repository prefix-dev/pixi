use crate::consts::TASK_STYLE;
use crate::lock_file::LockFileDerivedData;
use crate::project::Environment;
use crate::task::TaskName;
use crate::{
    task::task_graph::{TaskGraph, TaskId},
    task::{quote_arguments, Task},
    Project,
};
use deno_task_shell::{
    execute_with_pipes, parser::SequentialList, pipe, ShellPipeWriter, ShellState,
};
use itertools::Itertools;
use miette::Diagnostic;
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::{Display, Formatter},
    path::PathBuf,
};
use thiserror::Error;
use tokio::task::JoinHandle;

use super::task_hash::{InputHashesError, TaskCache, TaskHash};

/// Runs task in project.
#[derive(Default, Debug)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error, Diagnostic)]
#[error("deno task shell failed to parse '{script}': {error}")]
pub struct FailedToParseShellScript {
    pub script: String,
    pub error: String,
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

/// A task that contains enough information to be able to execute it. The lifetime [`'p`] refers to
/// the lifetime of the project that contains the tasks.
#[derive(Clone)]
pub struct ExecutableTask<'p> {
    pub project: &'p Project,
    pub name: Option<TaskName>,
    pub task: Cow<'p, Task>,
    pub run_environment: Environment<'p>,
    pub additional_args: Vec<String>,
}

impl<'p> ExecutableTask<'p> {
    /// Constructs a new executable task from a task graph node.
    pub fn from_task_graph(task_graph: &TaskGraph<'p>, task_id: TaskId) -> Self {
        let node = &task_graph[task_id];
        Self {
            project: task_graph.project(),
            name: node.name.clone(),
            task: node.task.clone(),
            run_environment: node.run_environment.clone(),
            additional_args: node.additional_args.clone(),
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

    /// Returns any additional args to pass to the execution of the task.
    pub fn additional_args(&self) -> &[String] {
        &self.additional_args
    }

    /// Returns the project in which this task is defined.
    pub fn project(&self) -> &'p Project {
        self.project
    }

    /// Returns a [`SequentialList`] which can be executed by deno task shell. Returns `None` if the
    /// command is not executable like in the case of an alias.
    pub fn as_deno_script(&self) -> Result<Option<SequentialList>, FailedToParseShellScript> {
        // Convert the task into an executable string
        let Some(task) = self.task.as_single_command() else {
            return Ok(None);
        };

        // Append the environment variables if they don't exist
        let mut export = String::new();
        if let Some(env) = self.task.env() {
            for (key, value) in env {
                if value.contains(format!("${}", key).as_str())
                    || std::env::var(key.as_str()).is_err()
                {
                    tracing::info!("Setting environment variable: {}={}", key, value);
                    export.push_str(&format!("export {}={};\n", key, value));
                }
                tracing::info!("Environment variable {} already set", key);
            }
        }

        // Append the command line arguments
        let cli_args = quote_arguments(self.additional_args.iter().map(|arg| arg.as_str()));
        let full_script = format!("{export}\n{task} {cli_args}");

        // Parse the shell command
        deno_task_shell::parser::parse(full_script.trim())
            .map_err(|e| FailedToParseShellScript {
                script: full_script,
                error: e.to_string(),
            })
            .map(Some)
    }

    /// Returns the working directory for this task.
    pub fn working_directory(&self) -> Result<PathBuf, InvalidWorkingDirectory> {
        Ok(match self.task.working_directory() {
            Some(cwd) if cwd.is_absolute() => cwd.to_path_buf(),
            Some(cwd) => {
                let abs_path = self.project.root().join(cwd);
                if !abs_path.is_dir() {
                    return Err(InvalidWorkingDirectory {
                        path: cwd.to_string_lossy().to_string(),
                    });
                }
                abs_path
            }
            None => self.project.root().to_path_buf(),
        })
    }

    /// Returns the full command that should be executed for this task. This includes any
    /// additional arguments that should be passed to the command.
    ///
    /// This function returns `None` if the task does not define a command to execute. This is the
    /// case for alias only commands.
    pub fn full_command(&self) -> Option<String> {
        let mut cmd = self.task.as_single_command()?.to_string();

        if !self.additional_args.is_empty() {
            cmd.push(' ');
            cmd.push_str(&self.additional_args.join(" "));
        }

        Some(cmd)
    }

    /// Returns an object that implements [`Display`] which outputs the command of the wrapped task.
    pub fn display_command(&self) -> impl Display + '_ {
        ExecutableTaskConsoleDisplay { task: self }
    }

    /// Executes the task and capture its output.
    pub async fn execute_with_pipes(
        &self,
        command_env: &HashMap<String, String>,
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
            stdin_writer.write_all(stdin).unwrap();
        }
        drop(stdin_writer); // prevent a deadlock by dropping the writer
        let (stdout, stdout_handle) = get_output_writer_and_handle();
        let (stderr, stderr_handle) = get_output_writer_and_handle();
        let state = ShellState::new(command_env.clone(), &cwd, Default::default());
        let code = execute_with_pipes(script, state, stdin, stdout, stderr).await;
        Ok(RunOutput {
            exit_code: code,
            stdout: stdout_handle.await.unwrap(),
            stderr: stderr_handle.await.unwrap(),
        })
    }

    /// We store the hashes of the inputs and the outputs of the task in a file in the cache.
    /// The current name is something like `run_environment-task_name.json`.
    pub(crate) fn cache_name(&self) -> String {
        format!(
            "{}-{}.json",
            self.run_environment.name(),
            self.name().unwrap_or("default")
        )
    }

    /// Checks if the task can be skipped. If the task can be skipped, it returns `CanSkip::Yes`.
    /// If the task cannot be skipped, it returns `CanSkip::No` and includes the hash of the task
    /// that caused the task to not be skipped - we can use this later to update the cache file quickly.
    pub(crate) async fn can_skip(
        &self,
        lock_file: &LockFileDerivedData<'_>,
    ) -> Result<CanSkip, std::io::Error> {
        tracing::info!("Checking if task can be skipped");
        let cache_name = self.cache_name();
        let cache_file = self.project().task_cache_folder().join(cache_name);
        if cache_file.exists() {
            let cache = tokio::fs::read_to_string(&cache_file).await?;
            let cache: TaskCache = serde_json::from_str(&cache)?;
            let hash = TaskHash::from_task(self, &lock_file.lock_file).await;
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

    /// Saves the cache of the task. This function will update the cache file with the new hash of
    /// the task (inputs and outputs). If the task has no hash, it will not save the cache.
    pub(crate) async fn save_cache(
        &self,
        lock_file: &LockFileDerivedData<'_>,
        previous_hash: Option<TaskHash>,
    ) -> Result<(), CacheUpdateError> {
        let task_cache_folder = self.project().task_cache_folder();
        let cache_file = task_cache_folder.join(self.cache_name());
        let new_hash = if let Some(mut previous_hash) = previous_hash {
            previous_hash.update_output(self).await?;
            previous_hash
        } else if let Some(hash) = TaskHash::from_task(self, &lock_file.lock_file).await? {
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

/// A helper object that implements [`Display`] to display (with ascii color) the command of the
/// task.
struct ExecutableTaskConsoleDisplay<'p, 't> {
    task: &'t ExecutableTask<'p>,
}

impl<'p, 't> Display for ExecutableTaskConsoleDisplay<'p, 't> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let command = self.task.task.as_single_command();
        write!(
            f,
            "{}",
            TASK_STYLE
                .apply_to(command.as_deref().unwrap_or("<alias>"))
                .bold()
        )?;
        if !self.task.additional_args.is_empty() {
            write!(
                f,
                " {}",
                TASK_STYLE.apply_to(self.task.additional_args.iter().format(" "))
            )?;
        }
        Ok(())
    }
}
/// Helper function to create a pipe that we can get the output from.
fn get_output_writer_and_handle() -> (ShellPipeWriter, JoinHandle<String>) {
    let (reader, writer) = pipe();
    let handle = reader.pipe_to_string_handle();
    (writer, handle)
}
