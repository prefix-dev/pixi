use crate::project;
use crate::task::{ExecutableTask, FileHashes, FileHashesError, InvalidWorkingDirectory};
use miette::Diagnostic;
use rattler_conda_types::Platform;
use rattler_lock::LockFile;
use serde::{Deserialize, Serialize};
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

impl ComputationHash {
    pub fn as_str(&self) -> &str {
        &self.0
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

#[derive(Debug, Hash)]
pub struct EnvironmentHash(String);

impl EnvironmentHash {
    fn from_environment(run_environment: &project::Environment<'_>, lock_file: &LockFile) -> Self {
        let mut hasher = Xxh3::new();
        let activation_scripts = run_environment.activation_scripts(Some(Platform::current()));

        for script in activation_scripts {
            script.hash(&mut hasher);
        }

        let mut urls = Vec::new();

        if let Some(env) = lock_file.environment(run_environment.name().as_str()) {
            if let Some(packages) = env.packages(Platform::current()) {
                for package in packages {
                    urls.push(package.url_or_path().into_owned().to_string())
                }
            }
        }

        urls.sort();

        urls.hash(&mut hasher);
        EnvironmentHash(format!("{:x}", hasher.finish()))
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
        let output_hashes = OutputHashes::from_task(task).await?;

        if input_hashes.is_none() && output_hashes.is_none() {
            return Ok(None);
        }

        Ok(Some(Self {
            command: task.full_command(),
            outputs: output_hashes,
            inputs: input_hashes,
            environment: EnvironmentHash::from_environment(&task.run_environment, lock_file),
        }))
    }

    pub async fn update_output(
        &mut self,
        task: &ExecutableTask<'_>,
    ) -> Result<(), InputHashesError> {
        self.outputs = OutputHashes::from_task(task).await?;
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

        let files = FileHashes::from_files(&task.project().root(), inputs.iter()).await?;
        Ok(Some(Self { files }))
    }
}

/// The combination of all the hashes of the inputs of a task.
#[derive(Debug, Hash)]
pub struct OutputHashes {
    pub files: FileHashes,
}

impl OutputHashes {
    /// Compute the input hashes from a task.
    pub async fn from_task(task: &ExecutableTask<'_>) -> Result<Option<Self>, InputHashesError> {
        let Some(ref outputs) = task.task().as_execute().and_then(|e| e.outputs.clone()) else {
            return Ok(None);
        };

        let files = FileHashes::from_files(&task.project().root(), outputs.iter()).await?;
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
