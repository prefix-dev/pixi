#![allow(dead_code)]

pub mod builders;
pub mod package_database;

use crate::common::builders::{AddBuilder, InitBuilder, TaskAddBuilder, TaskAliasBuilder};
use pixi::cli::install::Args;
use pixi::cli::run::{
    create_script, execute_script_with_output, get_task_env, order_tasks, RunOutput,
};
use pixi::cli::task::{AddArgs, AliasArgs};
use pixi::cli::{add, init, run, task};
use pixi::{consts, Project};
use rattler_conda_types::conda_lock::CondaLock;
use rattler_conda_types::{MatchSpec, Version};

use miette::IntoDiagnostic;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::str::FromStr;
use tempfile::TempDir;

/// To control the pixi process
pub struct PixiControl {
    /// The path to the project working file
    tmpdir: TempDir,
}

pub struct RunResult {
    output: Output,
}

impl RunResult {
    /// Was the output successful
    pub fn success(&self) -> bool {
        self.output.status.success()
    }

    /// Get the output
    pub fn stdout(&self) -> &str {
        std::str::from_utf8(&self.output.stdout).expect("could not get output")
    }
}

/// MatchSpecs from an iterator
pub fn string_from_iter(iter: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    iter.into_iter().map(|s| s.as_ref().to_string()).collect()
}

pub trait LockFileExt {
    /// Check if this package is contained in the lockfile
    fn contains_package(&self, name: impl AsRef<str>) -> bool;
    /// Check if this matchspec is contained in the lockfile
    fn contains_matchspec(&self, matchspec: impl AsRef<str>) -> bool;
}

impl LockFileExt for CondaLock {
    fn contains_package(&self, name: impl AsRef<str>) -> bool {
        self.package
            .iter()
            .any(|locked_dep| locked_dep.name == name.as_ref())
    }

    fn contains_matchspec(&self, matchspec: impl AsRef<str>) -> bool {
        let matchspec = MatchSpec::from_str(matchspec.as_ref()).expect("could not parse matchspec");
        let name = matchspec.name.expect("expected matchspec to have a name");
        let version = matchspec
            .version
            .expect("expected versionspec to have a name");
        self.package
            .iter()
            .find(|locked_dep| {
                let package_version =
                    Version::from_str(&locked_dep.version).expect("could not parse version");
                locked_dep.name == name && version.matches(&package_version)
            })
            .is_some()
    }
}

impl PixiControl {
    /// Create a new PixiControl instance
    pub fn new() -> miette::Result<PixiControl> {
        let tempdir = tempfile::tempdir().into_diagnostic()?;
        Ok(PixiControl { tmpdir: tempdir })
    }

    /// Loads the project manifest and returns it.
    pub fn project(&self) -> miette::Result<Project> {
        Project::load(&self.manifest_path())
    }

    /// Get the path to the project
    pub fn project_path(&self) -> &Path {
        self.tmpdir.path()
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.project_path().join(consts::PROJECT_MANIFEST)
    }

    /// Initialize pixi project inside a temporary directory. Returns a [`InitBuilder`]. To execute
    /// the command and await the result call `.await` on the return value.
    pub fn init(&self) -> InitBuilder {
        InitBuilder {
            args: init::Args {
                path: self.project_path().to_path_buf(),
                channels: Vec::new(),
            },
        }
    }

    /// Initialize pixi project inside a temporary directory. Returns a [`AddBuilder`]. To execute
    /// the command and await the result call `.await` on the return value.
    pub fn add(&self, spec: impl IntoMatchSpec) -> AddBuilder {
        AddBuilder {
            args: add::Args {
                manifest_path: Some(self.manifest_path()),
                host: false,
                specs: vec![spec.into()],
                build: false,
            },
        }
    }

    /// Run a command
    pub async fn run(&self, mut args: run::Args) -> miette::Result<RunOutput> {
        args.manifest_path = args.manifest_path.or_else(|| Some(self.manifest_path()));
        let mut tasks = order_tasks(args.task, &self.project().unwrap())?;

        let project = self.project().unwrap();
        let task_env = get_task_env(&project).await.unwrap();

        while let Some((command, args)) = tasks.pop_back() {
            let script = create_script(command, args).await;
            if let Ok(script) = script {
                let output = execute_script_with_output(script, &project, &task_env, None).await;
                if tasks.is_empty() {
                    return Ok(output);
                }
            }
        }
        Ok(RunOutput::default())
    }

    /// Create an installed environment. I.e a resolved and installed prefix
    pub async fn install(&self) -> miette::Result<()> {
        pixi::cli::install::execute(Args {
            manifest_path: Some(self.manifest_path()),
        })
        .await
    }

    /// Get the associated lock file
    pub async fn lock_file(&self) -> miette::Result<CondaLock> {
        pixi::environment::load_lock_for_manifest_path(&self.manifest_path()).await
    }

    pub fn tasks(&self) -> TasksControl {
        TasksControl { pixi: self }
    }
}

pub struct TasksControl<'a> {
    /// Reference to the pixi control
    pixi: &'a PixiControl,
}

impl TasksControl<'_> {
    /// Add a task
    pub fn add(&self, name: impl ToString) -> TaskAddBuilder {
        TaskAddBuilder {
            manifest_path: Some(self.pixi.manifest_path()),
            args: AddArgs {
                name: name.to_string(),
                commands: vec![],
                depends_on: None,
            },
        }
    }

    /// Remove a task
    pub async fn remove(&self, name: impl ToString) -> miette::Result<()> {
        task::execute(task::Args {
            manifest_path: Some(self.pixi.manifest_path()),
            operation: task::Operation::Remove(task::RemoveArgs {
                names: vec![name.to_string()],
            }),
        })
    }

    /// Alias one or multiple tasks
    pub fn alias(&self, name: impl ToString) -> TaskAliasBuilder {
        TaskAliasBuilder {
            manifest_path: Some(self.pixi.manifest_path()),
            args: AliasArgs {
                alias: name.to_string(),
                depends_on: vec![],
            },
        }
    }
}

/// A helper trait to convert from different types into a [`MatchSpec`] to make it simpler to
/// use them in tests.
pub trait IntoMatchSpec {
    fn into(self) -> MatchSpec;
}

impl IntoMatchSpec for &str {
    fn into(self) -> MatchSpec {
        MatchSpec::from_str(self).unwrap()
    }
}

impl IntoMatchSpec for String {
    fn into(self) -> MatchSpec {
        MatchSpec::from_str(&self).unwrap()
    }
}

impl IntoMatchSpec for MatchSpec {
    fn into(self) -> MatchSpec {
        self
    }
}
