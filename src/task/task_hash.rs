use crate::task::{ExecutableTask, FileHashes, FileHashesError, InvalidWorkingDirectory};
use crate::workspace;
use miette::Diagnostic;
use rattler_lock::LockFile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

/// The computation hash is a combined hash of all the inputs and outputs of a task.
///
/// Use a [`TaskHash`] to construct a computation hash.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct ComputationHash(String);

impl From<String> for ComputationHash {
    fn from(value: String) -> Self {
        ComputationHash(value)
    }
}

impl Display for ComputationHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The cache of a task. It contains the hash of the task.
#[derive(Deserialize, Serialize)]
pub struct TaskCache {
    /// The hash of the task.
    pub hash: ComputationHash,
}

#[derive(Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct EnvironmentHash(String);

impl EnvironmentHash {
    pub(crate) fn from_environment(
        run_environment: &workspace::Environment<'_>,
        input_environment_variables: &HashMap<String, Option<String>>,
        lock_file: &LockFile,
    ) -> Self {
        let mut hasher = Xxh3::new();

        // Hash the environment variables
        let mut sorted_input_environment_variables: Vec<_> =
            input_environment_variables.iter().collect();
        sorted_input_environment_variables.sort_by_key(|(key, _)| *key);
        for (key, value) in sorted_input_environment_variables {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
        }

        // Hash the activation scripts
        let activation_scripts =
            run_environment.activation_scripts(Some(run_environment.best_platform()));
        for script in activation_scripts {
            script.hash(&mut hasher);
        }

        // Hash the environment variables
        let project_activation_env =
            run_environment.activation_env(Some(run_environment.best_platform()));
        let mut env_vars: Vec<_> = project_activation_env.iter().collect();
        env_vars.sort_by_key(|(key, _)| *key);

        for (key, value) in env_vars {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
        }

        // Hash the packages
        let mut urls = Vec::new();
        if let Some(env) = lock_file.environment(run_environment.name().as_str()) {
            if let Some(packages) = env.packages(run_environment.best_platform()) {
                for package in packages {
                    urls.push(package.location().to_string())
                }
            }
        }
        urls.sort();
        urls.hash(&mut hasher);

        EnvironmentHash(format!("{:x}", hasher.finish()))
    }
}

impl Display for EnvironmentHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The [`TaskHash`] group all the hashes of a task. It can be converted to a [`ComputationHash`]
/// with the [`TaskHash::computation_hash`] method.
#[derive(Debug)]
pub struct TaskHash {
    pub environment: EnvironmentHash,
    pub command: Option<String>,
    pub inputs: Option<InputHashes>,
    pub outputs: Option<OutputHashes>,
}

impl TaskHash {
    /// Constructs an instance from an executable task.
    pub async fn from_task(
        task: &ExecutableTask<'_>,
        lock_file: &LockFile,
    ) -> Result<Option<Self>, InputHashesError> {
        let input_hashes = InputHashes::from_task(task).await?;
        let output_hashes = OutputHashes::from_task(task, false).await?;

        if input_hashes.is_none() && output_hashes.is_none() {
            return Ok(None);
        }

        Ok(Some(Self {
            command: task.full_command(),
            outputs: output_hashes,
            inputs: input_hashes,
            // Skipping environment variables used for caching the task
            environment: EnvironmentHash::from_environment(
                &task.run_environment,
                &HashMap::new(),
                lock_file,
            ),
        }))
    }

    pub async fn update_output(
        &mut self,
        task: &ExecutableTask<'_>,
    ) -> Result<(), InputHashesError> {
        self.outputs = OutputHashes::from_task(task, true).await?;
        Ok(())
    }

    /// Computes a single hash for the task.
    pub fn computation_hash(&self) -> ComputationHash {
        let mut hasher = Xxh3::new();
        self.command.hash(&mut hasher);
        self.inputs.hash(&mut hasher);
        self.outputs.hash(&mut hasher);
        self.environment.hash(&mut hasher);
        ComputationHash(format!("{:x}", hasher.finish()))
    }
}

/// The combination of all the hashes of the inputs of a task.
#[derive(Debug, Hash)]
pub struct InputHashes {
    pub files: FileHashes,
}

impl InputHashes {
    /// Compute the input hashes from a task.
    pub async fn from_task(task: &ExecutableTask<'_>) -> Result<Option<Self>, InputHashesError> {
        let Some(ref inputs) = task.task().as_execute().and_then(|e| e.inputs.clone()) else {
            return Ok(None);
        };

        let files = FileHashes::from_files(task.project().root(), inputs.iter()).await?;

        // check if any files were matched
        if files.files.is_empty() {
            tracing::warn!(
                "No files matched the input globs for task '{}'",
                task.name().unwrap_or_default()
            );
            tracing::warn!(
                "Input globs: {:?}",
                inputs.iter().map(|g| g.as_str()).collect::<Vec<_>>()
            );
        }

        Ok(Some(Self { files }))
    }
}

/// The combination of all the hashes of the inputs of a task.
#[derive(Debug, Hash)]
pub struct OutputHashes {
    pub files: FileHashes,
}

impl OutputHashes {
    /// Compute the output hashes from a task.
    pub async fn from_task(
        task: &ExecutableTask<'_>,
        warn: bool,
    ) -> Result<Option<Self>, InputHashesError> {
        let Some(ref outputs) = task.task().as_execute().and_then(|e| e.outputs.clone()) else {
            return Ok(None);
        };

        let files = FileHashes::from_files(task.project().root(), outputs.iter()).await?;

        // check if any files were matched
        if warn && files.files.is_empty() {
            tracing::warn!(
                "No files matched the output globs for task` '{}'",
                task.name().unwrap_or_default()
            );
            tracing::warn!(
                "Output globs: {:?}",
                outputs.iter().map(|g| g.as_str()).collect::<Vec<_>>()
            );
            return Ok(None);
        }

        Ok(Some(Self { files }))
    }
}

/// An error that might occur when computing the input hashes of a task.
#[derive(Debug, Error, Diagnostic)]
pub enum InputHashesError {
    #[error(transparent)]
    FileHashes(#[from] FileHashesError),

    #[error(transparent)]
    InvalidWorkingDirectory(#[from] InvalidWorkingDirectory),
}
