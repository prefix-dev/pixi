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

impl PixiControl {
    /// Create a new PixiControl instance
    pub fn new() -> anyhow::Result<PixiControl> {
        let tempdir = tempfile::tempdir()?;
        Ok(PixiControl {
            tmpdir: tempdir,
            project: None,
        })
    }

    pub fn project(&self) -> &Project {
        self.project.as_ref().expect("should call .init() first")
    }

    pub fn project_mut(&mut self) -> &mut Project {
        self.project.as_mut().expect("should call .init() first")
    }

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
    ) -> anyhow::Result<i32> {
        std::env::set_current_dir(self.project_path()).unwrap();
        run::execute_in_project(
            self.project(),
            command
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
            true,
        )
        .await
    }

    /// Get the associated lock file
    pub async fn lock_file(&self) -> anyhow::Result<CondaLock> {
        pixi::environment::load_lock_file(self.project()).await
    }
}

pub trait LockFileExt {
    fn contains_package(&self, name: impl AsRef<str>) -> bool;
    fn contains_matchspec(&self, matchspec: impl AsRef<str>) -> bool;
}

impl LockFileExt for CondaLock {
    fn contains_package(&self, name: impl AsRef<str>) -> bool {
        self.package
            .iter()
            .find(|locked_dep| locked_dep.name == name.as_ref())
            .is_some()
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
