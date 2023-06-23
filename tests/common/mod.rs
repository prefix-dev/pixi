use pixi::cli::{add, init, run};
use pixi::Project;
use rattler_conda_types::conda_lock::CondaLock;
use rattler_conda_types::{MatchSpec, Version};
use std::path::Path;
use std::str::FromStr;
use tempfile::TempDir;

/// To control the pixi process
pub struct PixiControl {
    /// The path to the project working file
    tmpdir: TempDir,

    /// The project that could be worked on
    project: Option<Project>,
}

pub struct RunResult {
    output: std::process::Output,
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

    /// Check if it matches specific output
    pub fn matches_output(&self, str: impl AsRef<str>) -> bool {
        self.stdout() == str.as_ref()
    }
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
    pub fn new() -> anyhow::Result<PixiControl> {
        let tempdir = tempfile::tempdir()?;
        Ok(PixiControl {
            tmpdir: tempdir,
            project: None,
        })
    }

    /// Access to the project
    pub fn project(&self) -> &Project {
        self.project.as_ref().expect("should call .init() first")
    }

    /// Mutable access to the project
    pub fn project_mut(&mut self) -> &mut Project {
        self.project.as_mut().expect("should call .init() first")
    }

    /// Get the path to the project
    pub fn project_path(&self) -> &Path {
        self.tmpdir.path()
    }

    /// Initialize pixi inside a tempdir and set the tempdir as the current working directory.
    pub async fn init(&mut self) -> anyhow::Result<()> {
        std::env::set_current_dir(self.project_path()).unwrap();
        let args = init::Args {
            path: self.project_path().to_path_buf(),
        };
        init::execute(args).await?;
        self.project = Some(Project::discover()?);
        Ok(())
    }

    /// Add a dependency to the project
    pub async fn add(
        &mut self,
        specs: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> anyhow::Result<()> {
        std::env::set_current_dir(self.project_path()).unwrap();
        add::add_specs_to_project(
            self.project_mut(),
            specs
                .into_iter()
                .map(|s| s.as_ref().parse())
                .collect::<Result<Vec<_>, _>>()?,
        )
        .await
    }

    /// Run a command
    pub async fn run(
        &self,
        command: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> anyhow::Result<RunResult> {
        std::env::set_current_dir(self.project_path()).unwrap();
        let output = run::execute_in_project_with_output(
            self.project(),
            command
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        )
        .await?;
        Ok(RunResult { output })
    }

    /// Get the associated lock file
    pub async fn lock_file(&self) -> anyhow::Result<CondaLock> {
        pixi::environment::load_lock_file(self.project()).await
    }
}
