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
        let output_hashes = OutputHashes::from_task(task).await?;

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
        self.outputs = OutputHashes::from_task(task).await?;
        Ok(())
    }

    pub async fn update_input(
        &mut self,
        task: &ExecutableTask<'_>,
    ) -> Result<(), InputHashesError> {
        self.inputs = InputHashes::from_task(task).await?;
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

        // Always hash the args if they exist
        task.args().hash(&mut hasher);
        let context = task.render_context();

        // Hash inputs if present (on Alias or Execute)
        if let Some(inputs) = task.task().inputs() {
            let rendered_inputs = inputs.render(&context)?;
            rendered_inputs.hash(&mut hasher);
        }

        // Hash outputs if present
        if let Some(outputs) = task.task().outputs() {
            let rendered_outputs = outputs.render(&context)?;
            rendered_outputs.hash(&mut hasher);
        }

        Ok(Some(NameHash::from(&hasher as &dyn Hasher)))
    }
}

/// The combination of all the hashes of the inputs of a task.
#[derive(Debug, Hash)]
pub struct InputHashes {
    pub files: FileHashes,
}

impl InputHashes {
    /// Compute the input hashes from a task. Returns `None` if no files match.
    pub async fn from_task(task: &ExecutableTask<'_>) -> Result<Option<Self>, InputHashesError> {
        // Use .inputs() directly from the task (works for Alias and Execute)
        let Some(inputs) = task.task().inputs() else {
            return Ok(None);
        };

        if inputs.is_empty() {
            return Ok(None);
        }

        let context = task.render_context();
        let rendered_inputs: Vec<String> = inputs
            .iter()
            .map(|i| i.render(&context))
            .collect::<Result<_, _>>()?;

        let files = FileHashes::from_files(task.project().root(), &rendered_inputs).await?;

        // If no files matched, treat as no inputs for caching purposes
        if files.files.is_empty() {
            return Ok(None);
        }

        Ok(Some(Self { files }))
    }
}

#[derive(Debug, Hash)]
pub struct OutputHashes {
    pub files: FileHashes,
}

impl OutputHashes {
    /// Compute the output hashes from a task. Returns `None` if no files match.
    pub async fn from_task(task: &ExecutableTask<'_>) -> Result<Option<Self>, InputHashesError> {
        // Use .outputs() directly (works for Alias and Execute)
        let Some(outputs) = task.task().outputs() else {
            return Ok(None);
        };

        let context = task.render_context();
        let mut rendered_outputs = Vec::new();
        for output in outputs.iter() {
            match output.render(&context) {
                Ok(rendered) => rendered_outputs.push(rendered),
                Err(err) => return Err(InputHashesError::TemplateStringError(err)),
            }
        }

        if rendered_outputs.is_empty() {
            return Ok(None);
        }

        // Use our rendered list to check the filesystem
        let files = FileHashes::from_files(task.project().root(), rendered_outputs.iter()).await?;

        if files.files.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_core::Workspace;
    use pixi_manifest::TaskName;
    use std::borrow::Cow;

    #[tokio::test]
    async fn test_alias_task_hash_with_outputs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_path = temp_dir.path().join("pixi.toml");

        // 1. Define a workspace with an alias task that has outputs
        let manifest = r#"
            [project]
            name = "alias_test"
            version = "0.1.0"
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64"]

            [tasks]
            # An alias task (no cmd) with an output glob
            my_alias = { depends-on = [], outputs = ["output.txt"] }
        "#;
        std::fs::write(&project_path, manifest).unwrap();

        // 2. Create the dummy output file so the glob matches
        std::fs::write(temp_dir.path().join("output.txt"), "hello world").unwrap();

        let workspace = Workspace::from_path(&project_path).unwrap();
        let task_name = TaskName::from("my_alias");
        let environment = workspace.default_environment();
        let task = environment.task(&task_name, None).unwrap();

        let executable_task = ExecutableTask {
            workspace: &workspace,
            name: Some(task_name),
            task: Cow::Borrowed(task),
            run_environment: environment,
            args: Default::default(),
        };

        // 3. Compute the hash
        let lock_file = rattler_lock::LockFile::default();
        let hash = TaskHash::from_task(&executable_task, &lock_file)
            .await
            .unwrap()
            .expect("TaskHash should not be None even for alias tasks with outputs");

        // 4. Assertions
        assert!(hash.command.is_none());
        assert!(
            hash.outputs.is_some(),
            "Outputs should be captured for alias tasks"
        );

        let output_hashes = &hash.outputs.as_ref().unwrap().files;
        // Use Path::new() to satisfy the Borrow<Path> requirement of the HashMap
        assert!(
            output_hashes
                .files
                .contains_key(std::path::Path::new("output.txt")),
            "output.txt should be in the hash"
        );
    }
}
