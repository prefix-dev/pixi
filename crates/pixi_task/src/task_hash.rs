use miette::Diagnostic;
use pixi_core::environment::EnvironmentHash;
use pixi_manifest::task::TemplateStringError;
use rattler_lock::LockFile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

use crate::{ExecutableTask, FileHashes, FileHashesError, InvalidWorkingDirectory};

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

/// The name hash is a combined hash of all the inputs and outputs of a task.
/// and it's used as a name for the task cache file.
///
/// Use a [`TaskHash`] to construct a name hash.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct NameHash(String);

impl From<String> for NameHash {
    fn from(value: String) -> Self {
        NameHash(value)
    }
}

impl From<&dyn Hasher> for NameHash {
    fn from(hasher: &dyn Hasher) -> Self {
        NameHash(format!("{:x}", hasher.finish()))
    }
}

impl Display for NameHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The cache of a task. It contains the hash of the task.
#[derive(Deserialize, Serialize, Debug)]
pub struct TaskCache {
    /// The hash of the task.
    pub hash: ComputationHash,
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
            command: task.full_command().ok().flatten(),
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

    /// Return the hash that should be used as the name of the task cache file.
    /// It takes the rendered inputs and rendered outputs of the task into account.
    pub fn task_args_hash(task: &ExecutableTask<'_>) -> Result<Option<NameHash>, InputHashesError> {
        let mut hasher = Xxh3::new();

        let Ok(execute) = task.task().as_execute() else {
            return Ok(None);
        };

        // We need to compute hash from input args
        // If no input args are provided, we treat them as empty list.
        if let Some(ref inputs) = execute.inputs {
            let rendered_inputs = inputs.render(Some(task.args()))?;
            rendered_inputs.hash(&mut hasher);
        }

        // and the same for output args
        if let Some(ref outputs) = execute.outputs {
            let rendered_outputs = outputs.render(Some(task.args()))?;
            rendered_outputs.hash(&mut hasher);
        }

        // Create a namehash from the hasher
        Ok(Some(NameHash::from(&hasher as &dyn Hasher)))
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
        let Ok(execute) = task.task().as_execute() else {
            return Ok(None);
        };

        let Some(inputs) = &execute.inputs else {
            return Ok(None);
        };

        if inputs.is_empty() {
            return Ok(None);
        }

        let rendered_inputs: Vec<String> = inputs
            .iter()
            .map(|i| i.render(Some(task.args())))
            .collect::<Result<_, _>>()?;

        let files = FileHashes::from_files(task.project().root(), &rendered_inputs).await?;

        // check if any files were matched
        if files.files.is_empty() {
            tracing::warn!(
                "No files matched the input globs for task '{}'",
                task.name().unwrap_or_default()
            );
            tracing::warn!(
                "Input globs: {:?}",
                rendered_inputs
                    .iter()
                    .map(|g| g.as_str())
                    .collect::<Vec<_>>()
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
        let outputs: Vec<String> = match task.task().as_execute() {
            Ok(execute) => {
                if let Some(outputs) = execute.outputs.clone() {
                    let mut rendered_outputs = Vec::new();
                    for output in outputs.iter() {
                        match output.render(Some(task.args())) {
                            Ok(rendered) => rendered_outputs.push(rendered),
                            Err(err) => return Err(InputHashesError::TemplateStringError(err)),
                        }
                    }
                    if rendered_outputs.is_empty() {
                        return Ok(None);
                    }
                    rendered_outputs
                } else {
                    return Ok(None);
                }
            }
            Err(_) => return Ok(None),
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

    #[error(transparent)]
    #[diagnostic(transparent)]
    TemplateStringError(#[from] TemplateStringError),
}
