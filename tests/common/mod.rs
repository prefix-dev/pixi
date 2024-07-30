#![allow(dead_code)]

pub mod builders;
pub mod package_database;

use std::{
    path::{Path, PathBuf},
    process::Output,
    str::FromStr,
};

use self::builders::{HasDependencyConfig, RemoveBuilder};
use crate::common::builders::{
    AddBuilder, InitBuilder, InstallBuilder, ProjectChannelAddBuilder,
    ProjectEnvironmentAddBuilder, TaskAddBuilder, TaskAliasBuilder, UpdateBuilder,
};
use miette::{Context, Diagnostic, IntoDiagnostic};
use pixi::cli::cli_config::{PrefixUpdateConfig, ProjectConfig};
use pixi::task::{
    ExecutableTask, RunOutput, SearchEnvironments, TaskExecutionError, TaskGraph, TaskGraphError,
};
use pixi::{
    cli::{
        add, init,
        install::Args,
        project, remove, run,
        run::get_task_env,
        task::{self, AddArgs, AliasArgs},
        update, LockFileUsageArgs,
    },
    task::TaskName,
    Project, UpdateLockFileOptions,
};
use pixi_consts::consts;
use pixi_manifest::{EnvironmentName, FeatureName};
use rattler_conda_types::{MatchSpec, ParseStrictness::Lenient, Platform};
use rattler_lock::{LockFile, Package};
use tempfile::TempDir;
use thiserror::Error;

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
    fn contains_conda_package(&self, environment: &str, platform: Platform, name: &str) -> bool;
    fn contains_pypi_package(&self, environment: &str, platform: Platform, name: &str) -> bool;
    /// Check if this matchspec is contained in the lockfile
    fn contains_match_spec(
        &self,
        environment: &str,
        platform: Platform,
        match_spec: impl IntoMatchSpec,
    ) -> bool;

    /// Check if the pep508 requirement is contained in the lockfile for this
    /// platform
    fn contains_pep508_requirement(
        &self,
        environment: &str,
        platform: Platform,
        requirement: pep508_rs::Requirement,
    ) -> bool;
}

impl LockFileExt for LockFile {
    fn contains_conda_package(&self, environment: &str, platform: Platform, name: &str) -> bool {
        let Some(env) = self.environment(environment) else {
            return false;
        };
        let package_found = env
            .packages(platform)
            .into_iter()
            .flatten()
            .filter_map(Package::into_conda)
            .any(|package| package.package_record().name.as_normalized() == name);
        package_found
    }
    fn contains_pypi_package(&self, environment: &str, platform: Platform, name: &str) -> bool {
        let Some(env) = self.environment(environment) else {
            return false;
        };
        let package_found = env
            .packages(platform)
            .into_iter()
            .flatten()
            .filter_map(Package::into_pypi)
            .any(|pkg| pkg.data().package.name.as_ref() == name);
        package_found
    }

    fn contains_match_spec(
        &self,
        environment: &str,
        platform: Platform,
        match_spec: impl IntoMatchSpec,
    ) -> bool {
        let match_spec = match_spec.into();
        let Some(env) = self.environment(environment) else {
            return false;
        };
        let package_found = env
            .packages(platform)
            .into_iter()
            .flatten()
            .filter_map(Package::into_conda)
            .any(move |p| p.satisfies(&match_spec));
        package_found
    }

    fn contains_pep508_requirement(
        &self,
        environment: &str,
        platform: Platform,
        requirement: pep508_rs::Requirement,
    ) -> bool {
        let Some(env) = self.environment(environment) else {
            return false;
        };
        let package_found = env
            .packages(platform)
            .into_iter()
            .flatten()
            .filter_map(Package::into_pypi)
            .any(move |p| p.satisfies(&requirement));
        package_found
    }
}

impl PixiControl {
    /// Create a new PixiControl instance
    pub fn new() -> miette::Result<PixiControl> {
        let tempdir = tempfile::tempdir().into_diagnostic()?;
        Ok(PixiControl { tmpdir: tempdir })
    }

    /// Creates a new PixiControl instance from an existing manifest
    pub fn from_manifest(manifest: &str) -> miette::Result<PixiControl> {
        let pixi = Self::new()?;
        std::fs::write(pixi.manifest_path(), manifest)
            .into_diagnostic()
            .context("failed to write pixi.toml")?;
        Ok(pixi)
    }

    /// Updates the complete manifest
    pub fn update_manifest(&self, manifest: &str) -> miette::Result<()> {
        std::fs::write(self.manifest_path(), manifest)
            .into_diagnostic()
            .context("failed to write pixi.toml")?;
        Ok(())
    }

    /// Loads the project manifest and returns it.
    pub fn project(&self) -> miette::Result<Project> {
        Project::load_or_else_discover(Some(&self.manifest_path()))
    }

    /// Get the path to the project
    pub fn project_path(&self) -> &Path {
        self.tmpdir.path()
    }

    /// Get path to default environment
    pub fn default_env_path(&self) -> miette::Result<PathBuf> {
        let project = self.project()?;
        let env = project.environment("default");
        let env = env.ok_or_else(|| miette::miette!("default environment not found"))?;
        Ok(self.tmpdir.path().join(env.dir()))
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.project_path().join(consts::PROJECT_MANIFEST)
    }

    /// Initialize pixi project inside a temporary directory. Returns a
    /// [`InitBuilder`]. To execute the command and await the result call
    /// `.await` on the return value.
    pub fn init(&self) -> InitBuilder {
        InitBuilder {
            no_fast_prefix: false,
            args: init::Args {
                path: self.project_path().to_path_buf(),
                channels: None,
                platforms: Vec::new(),
                env_file: None,
                pyproject: false,
            },
        }
    }

    /// Initialize pixi project inside a temporary directory. Returns a
    /// [`InitBuilder`]. To execute the command and await the result call
    /// `.await` on the return value.
    pub fn init_with_platforms(&self, platforms: Vec<String>) -> InitBuilder {
        InitBuilder {
            no_fast_prefix: false,
            args: init::Args {
                path: self.project_path().to_path_buf(),
                channels: None,
                platforms,
                env_file: None,
                pyproject: false,
            },
        }
    }

    /// Add a dependency to the project. Returns an [`AddBuilder`].
    /// the command and await the result call `.await` on the return value.
    pub fn add(&self, spec: &str) -> AddBuilder {
        self.add_multiple(vec![spec])
    }

    /// Add dependencies to the project. Returns an [`AddBuilder`].
    /// the command and await the result call `.await` on the return value.
    pub fn add_multiple(&self, specs: Vec<&str>) -> AddBuilder {
        AddBuilder {
            args: add::Args {
                project_config: ProjectConfig {
                    manifest_path: Some(self.manifest_path()),
                },
                dependency_config: AddBuilder::dependency_config_with_specs(specs),
                prefix_update_config: PrefixUpdateConfig {
                    no_lockfile_update: false,
                    no_install: true,
                    config: Default::default(),
                },
                editable: false,
            },
        }
    }

    /// Remove dependencies from the project. Returns a [`RemoveBuilder`].
    pub fn remove(&self, spec: &str) -> RemoveBuilder {
        RemoveBuilder {
            args: remove::Args {
                project_config: ProjectConfig {
                    manifest_path: Some(self.manifest_path()),
                },
                dependency_config: AddBuilder::dependency_config_with_specs(vec![spec]),
                prefix_update_config: PrefixUpdateConfig {
                    no_lockfile_update: false,
                    no_install: true,
                    config: Default::default(),
                },
            },
        }
    }

    /// Add a new channel to the project.
    pub fn project_channel_add(&self) -> ProjectChannelAddBuilder {
        ProjectChannelAddBuilder {
            manifest_path: Some(self.manifest_path()),
            args: project::channel::AddRemoveArgs {
                channel: vec![],
                no_install: true,
                feature: None,
            },
        }
    }

    pub fn project_environment_add(&self, name: &str) -> ProjectEnvironmentAddBuilder {
        ProjectEnvironmentAddBuilder {
            manifest_path: Some(self.manifest_path()),
            args: project::environment::add::Args {
                name: name.to_string(),
                features: None,
                solve_group: None,
                no_default_feature: false,
                force: false,
            },
        }
    }

    /// Run a command
    pub async fn run(&self, mut args: run::Args) -> miette::Result<RunOutput> {
        args.project_config.manifest_path = args
            .project_config
            .manifest_path
            .or_else(|| Some(self.manifest_path()));

        // Load the project
        let project = self.project()?;

        // Extract the passed in environment name.
        let explicit_environment = args
            .environment
            .map(|n| EnvironmentName::from_str(n.as_str()))
            .transpose()?
            .map(|n| {
                project
                    .environment(&n)
                    .ok_or_else(|| miette::miette!("unknown environment '{n}'"))
            })
            .transpose()?;

        // Ensure the lock-file is up-to-date
        let mut lock_file = project
            .up_to_date_lock_file(UpdateLockFileOptions {
                lock_file_usage: args.lock_file_usage.into(),
                ..UpdateLockFileOptions::default()
            })
            .await?;

        // Create a task graph from the command line arguments.
        let search_env = SearchEnvironments::from_opt_env(
            &project,
            explicit_environment.clone(),
            explicit_environment
                .as_ref()
                .map(|e| e.best_platform())
                .or(Some(Platform::current())),
        );
        let task_graph = TaskGraph::from_cmd_args(&project, &search_env, args.task)
            .map_err(RunError::TaskGraphError)?;

        // Iterate over all tasks in the graph and execute them.
        let mut task_env = None;
        let mut result = RunOutput::default();
        for task_id in task_graph.topological_order() {
            let task = ExecutableTask::from_task_graph(&task_graph, task_id);

            // Construct the task environment if not already created.
            let task_env = match task_env.as_ref() {
                None => {
                    let env =
                        get_task_env(&mut lock_file, &task.run_environment, args.clean_env).await?;
                    task_env.insert(env)
                }
                Some(task_env) => task_env,
            };

            let output = task.execute_with_pipes(task_env, None).await?;
            result.stdout.push_str(&output.stdout);
            result.stderr.push_str(&output.stderr);
            result.exit_code = output.exit_code;
            if output.exit_code != 0 {
                return Err(RunError::NonZeroExitCode(output.exit_code).into());
            }
        }

        Ok(result)
    }

    /// Returns a [`InstallBuilder`]. To execute the command and await the
    /// result call `.await` on the return value.
    pub fn install(&self) -> InstallBuilder {
        InstallBuilder {
            args: Args {
                environment: None,
                project_config: ProjectConfig {
                    manifest_path: Some(self.manifest_path()),
                },
                lock_file_usage: LockFileUsageArgs {
                    frozen: false,
                    locked: false,
                },
                config: Default::default(),
                all: false,
            },
        }
    }

    /// Returns a [`UpdateBuilder]. To execute the command and await the result
    /// call `.await` on the return value.
    pub fn update(&self) -> UpdateBuilder {
        UpdateBuilder {
            args: update::Args {
                config: Default::default(),
                project_config: ProjectConfig {
                    manifest_path: Some(self.manifest_path()),
                },
                no_install: true,
                dry_run: false,
                specs: Default::default(),
                json: false,
            },
        }
    }

    /// Load the current lock-file.
    ///
    /// If you want to lock-file to be up-to-date with the project call
    /// [`Self::up_to_date_lock_file`].
    pub async fn lock_file(&self) -> miette::Result<LockFile> {
        let project = Project::load_or_else_discover(Some(&self.manifest_path()))?;
        pixi::load_lock_file(&project).await
    }

    /// Load the current lock-file and makes sure that its up to date with the
    /// project.
    pub async fn up_to_date_lock_file(&self) -> miette::Result<LockFile> {
        let project = self.project()?;
        Ok(project
            .up_to_date_lock_file(UpdateLockFileOptions::default())
            .await?
            .lock_file)
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
    pub fn add(
        &self,
        name: TaskName,
        platform: Option<Platform>,
        feature_name: FeatureName,
    ) -> TaskAddBuilder {
        let feature = feature_name.name().map(|s| s.to_string());
        TaskAddBuilder {
            manifest_path: Some(self.pixi.manifest_path()),
            args: AddArgs {
                name,
                commands: vec![],
                depends_on: None,
                platform,
                feature,
                cwd: None,
                env: Default::default(),
                description: None,
                clean_env: false,
            },
        }
    }

    /// Remove a task
    pub async fn remove(
        &self,
        name: TaskName,
        platform: Option<Platform>,
        feature_name: Option<String>,
    ) -> miette::Result<()> {
        task::execute(task::Args {
            project_config: ProjectConfig {
                manifest_path: Some(self.pixi.manifest_path()),
            },
            operation: task::Operation::Remove(task::RemoveArgs {
                names: vec![name],
                platform,
                feature: feature_name,
            }),
        })
    }

    /// Alias one or multiple tasks
    pub fn alias(&self, name: TaskName, platform: Option<Platform>) -> TaskAliasBuilder {
        TaskAliasBuilder {
            manifest_path: Some(self.pixi.manifest_path()),
            args: AliasArgs {
                platform,
                alias: name,
                depends_on: vec![],
                description: None,
            },
        }
    }
}

/// A helper trait to convert from different types into a [`MatchSpec`] to make
/// it simpler to use them in tests.
pub trait IntoMatchSpec {
    fn into(self) -> MatchSpec;
}

impl IntoMatchSpec for &str {
    fn into(self) -> MatchSpec {
        MatchSpec::from_str(self, Lenient).unwrap()
    }
}

impl IntoMatchSpec for String {
    fn into(self) -> MatchSpec {
        MatchSpec::from_str(&self, Lenient).unwrap()
    }
}

impl IntoMatchSpec for MatchSpec {
    fn into(self) -> MatchSpec {
        self
    }
}

#[derive(Error, Debug, Diagnostic)]
enum RunError {
    #[error(transparent)]
    TaskGraphError(#[from] TaskGraphError),
    #[error(transparent)]
    ExecutionError(#[from] TaskExecutionError),
    #[error("the task executed with a non-zero exit code {0}")]
    NonZeroExitCode(i32),
}
