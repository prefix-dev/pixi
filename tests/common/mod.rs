use pixi::cli::run::create_command;
use pixi::cli::{add, init, run};
use pixi::consts;
use rattler_conda_types::conda_lock::CondaLock;
use rattler_conda_types::{MatchSpec, Version};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;
use tempfile::TempDir;

/// To control the pixi process
pub struct PixiControl {
    /// The path to the project working file
    tmpdir: TempDir,
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
}

/// MatchSpecs from an iterator
pub fn matchspec_from_iter(iter: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<MatchSpec> {
    iter.into_iter()
        .map(|s| MatchSpec::from_str(s.as_ref()).expect("could not parse matchspec"))
        .collect()
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
    pub fn new() -> anyhow::Result<PixiControl> {
        let tempdir = tempfile::tempdir()?;
        Ok(PixiControl { tmpdir: tempdir })
    }

    /// Get the path to the project
    pub fn project_path(&self) -> &Path {
        self.tmpdir.path()
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.project_path().join(consts::PROJECT_MANIFEST)
    }

    /// Initialize pixi inside a tempdir and set the tempdir as the current working directory.
    pub async fn init(&self) -> anyhow::Result<()> {
        let args = init::Args {
            path: self.project_path().to_path_buf(),
        };
        init::execute(args).await?;
        Ok(())
    }

    /// Add a dependency to the project
    pub async fn add(&mut self, mut args: add::Args) -> anyhow::Result<()> {
        args.manifest_path = Some(self.manifest_path());
        add::execute(args).await
    }

    /// Run a command
    pub async fn run(&self, mut args: run::Args) -> anyhow::Result<RunResult> {
        args.manifest_path = Some(self.manifest_path());
        let mut script_command = create_command(args).await?;
        let output = script_command
            .command
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        Ok(RunResult { output })
    }

    /// Get the associated lock file
    pub async fn lock_file(&self) -> anyhow::Result<CondaLock> {
        pixi::environment::load_lock_for_manifest_path(&self.manifest_path()).await
    }
}
