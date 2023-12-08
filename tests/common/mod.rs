#![allow(dead_code)]

pub mod builders;
pub mod package_database;

use crate::common::builders::{
    AddBuilder, InitBuilder, InstallBuilder, ProjectChannelAddBuilder, TaskAddBuilder,
    TaskAliasBuilder,
};
use pixi::cli::install::Args;
use pixi::cli::run::{
    create_script, execute_script_with_output, get_task_env, order_tasks, RunOutput,
};
use pixi::cli::task::{AddArgs, AliasArgs};
use pixi::cli::{add, init, project, run, task};
use pixi::{consts, Project};
use rattler_conda_types::{MatchSpec, PackageName, Platform, Version};
use rattler_lock::{CondaLock, LockedDependencyKind};
use std::collections::HashSet;

use miette::IntoDiagnostic;
use pep508_rs::VersionOrUrl;
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
    fn contains_package(&self, name: &PackageName) -> bool;
    /// Check if this matchspec is contained in the lockfile
    fn contains_matchspec(&self, matchspec: impl IntoMatchSpec) -> bool;
    /// Check if this matchspec is contained in the lockfile for this platform
    fn contains_matchspec_for_platform(
        &self,
        matchspec: impl IntoMatchSpec,
        platform: impl Into<Platform>,
    ) -> bool;
    /// Check if the pep508 requirement is contained in the lockfile for this platform
    fn contains_pep508_requirement_for_platform(
        &self,
        requirement: pep508_rs::Requirement,
        platform: impl Into<Platform>,
    ) -> bool;
}

impl LockFileExt for CondaLock {
    fn contains_package(&self, name: &PackageName) -> bool {
        self.package
            .iter()
            .any(|locked_dep| locked_dep.name == *name.as_normalized())
    }

    fn contains_matchspec(&self, matchspec: impl IntoMatchSpec) -> bool {
        let matchspec = matchspec.into();
        let name = matchspec.name.expect("expected matchspec to have a name");
        let version = matchspec
            .version
            .expect("expected versionspec to have a name");
        self.package.iter().any(|locked_dep| {
            let package_version =
                Version::from_str(&locked_dep.version).expect("could not parse version");
            locked_dep.name == name.as_normalized() && version.matches(&package_version)
        })
    }

    fn contains_matchspec_for_platform(
        &self,
        matchspec: impl IntoMatchSpec,
        platform: impl Into<Platform>,
    ) -> bool {
        let matchspec = matchspec.into();
        let name = matchspec.name.expect("expected matchspec to have a name");
        let version = matchspec
            .version
            .expect("expected versionspec to have a name");
        let platform = platform.into();
        self.package.iter().any(|locked_dep| {
            let package_version =
                Version::from_str(&locked_dep.version).expect("could not parse version");
            locked_dep.name == name.as_normalized()
                && version.matches(&package_version)
                && locked_dep.platform == platform
        })
    }

    fn contains_pep508_requirement_for_platform(
        &self,
        requirement: pep508_rs::Requirement,
        platform: impl Into<Platform>,
    ) -> bool {
        let name = requirement.name;
        let version: Option<pep440_rs::VersionSpecifiers> =
            requirement
                .version_or_url
                .and_then(|version_or_url| match version_or_url {
                    VersionOrUrl::VersionSpecifier(version) => Some(version),
                    VersionOrUrl::Url(_) => unimplemented!(),
                });

        let platform = platform.into();
        self.package.iter().any(|locked_dep| {
            let package_version =
                pep440_rs::Version::from_str(&locked_dep.version).expect("could not parse version");

            let req_extras = requirement
                .extras
                .as_ref()
                .map(|extras| extras.iter().cloned().collect::<HashSet<_>>())
                .unwrap_or_default();

            locked_dep.name == *name
                && version
                    .as_ref()
                    .map_or(true, |v| v.contains(&package_version))
                && locked_dep.platform == platform
                // Check if the extras are the same.
                && match &locked_dep.kind {
                    LockedDependencyKind::Conda(_) => false,
                    LockedDependencyKind::Pypi(locked) => {
                        req_extras == locked.extras.iter().cloned().collect()
                    }
                }
        })
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
                channels: None,
                platforms: Vec::new(),
            },
        }
    }

    /// Initialize pixi project inside a temporary directory. Returns a [`InitBuilder`]. To execute
    /// the command and await the result call `.await` on the return value.
    pub fn init_with_platforms(&self, platforms: Vec<String>) -> InitBuilder {
        InitBuilder {
            args: init::Args {
                path: self.project_path().to_path_buf(),
                channels: None,
                platforms,
            },
        }
    }

    /// Initialize pixi project inside a temporary directory. Returns a [`AddBuilder`]. To execute
    /// the command and await the result call `.await` on the return value.
    pub fn add(&self, spec: &str) -> AddBuilder {
        AddBuilder {
            args: add::Args {
                manifest_path: Some(self.manifest_path()),
                host: false,
                specs: vec![spec.to_string()],
                build: false,
                no_install: true,
                no_lockfile_update: false,
                platform: Default::default(),
                pypi: false,
                sdist_resolution: Default::default(),
            },
        }
    }

    /// Add a new channel to the project.
    pub fn project_channel_add(&self) -> ProjectChannelAddBuilder {
        ProjectChannelAddBuilder {
            manifest_path: Some(self.manifest_path()),
            args: project::channel::add::Args {
                channel: vec![],
                no_install: true,
            },
        }
    }

    /// Run a command
    pub async fn run(&self, mut args: run::Args) -> miette::Result<RunOutput> {
        args.manifest_path = args.manifest_path.or_else(|| Some(self.manifest_path()));
        let mut tasks = order_tasks(args.task, &self.project().unwrap())?;

        let project = self.project().unwrap();
        let task_env = get_task_env(&project, args.frozen, args.locked)
            .await
            .unwrap();

        let mut result = RunOutput::default();
        while let Some((command, args)) = tasks.pop_back() {
            let cwd = run::select_cwd(command.working_directory(), &project)?;
            let script = create_script(&command, &args).await;
            if let Ok(script) = script {
                let output =
                    execute_script_with_output(script, cwd.as_path(), &task_env, None).await;
                result.stdout.push_str(&output.stdout);
                result.stderr.push_str(&output.stderr);
                result.exit_code = output.exit_code;
                if output.exit_code != 0 {
                    break;
                }
            }
        }
        Ok(result)
    }

    /// Returns a [`InstallBuilder`]. To execute the command and await the result call `.await` on the return value.
    pub fn install(&self) -> InstallBuilder {
        InstallBuilder {
            args: Args {
                manifest_path: Some(self.manifest_path()),
                locked: false,
                frozen: false,
            },
        }
    }

    /// Get the associated lock file
    pub async fn lock_file(&self) -> miette::Result<CondaLock> {
        let project = Project::load(&self.manifest_path())?;
        pixi::lock_file::load_lock_file(&project).await
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
    pub fn add(&self, name: impl ToString, platform: Option<Platform>) -> TaskAddBuilder {
        TaskAddBuilder {
            manifest_path: Some(self.pixi.manifest_path()),
            args: AddArgs {
                name: name.to_string(),
                commands: vec![],
                depends_on: None,
                platform,
                cwd: None,
            },
        }
    }

    /// Remove a task
    pub async fn remove(
        &self,
        name: impl ToString,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        task::execute(task::Args {
            manifest_path: Some(self.pixi.manifest_path()),
            operation: task::Operation::Remove(task::RemoveArgs {
                names: vec![name.to_string()],
                platform,
            }),
        })
    }

    /// Alias one or multiple tasks
    pub fn alias(&self, name: impl ToString, platform: Option<Platform>) -> TaskAliasBuilder {
        TaskAliasBuilder {
            manifest_path: Some(self.pixi.manifest_path()),
            args: AliasArgs {
                platform,
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
