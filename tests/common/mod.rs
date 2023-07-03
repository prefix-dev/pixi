#![allow(dead_code)]

pub mod package_database;

use pixi::cli::add::SpecType;
use pixi::cli::install::Args;
use pixi::cli::run::{create_command, get_command_env, order_commands};
use pixi::cli::{add, init, run};
use pixi::{consts, Project};
use rattler_conda_types::conda_lock::CondaLock;
use rattler_conda_types::{MatchSpec, Version};
use std::future::{Future, IntoFuture};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Output, Stdio};
use std::str::FromStr;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::spawn_blocking;
use url::Url;

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
    pub fn new() -> anyhow::Result<PixiControl> {
        let tempdir = tempfile::tempdir()?;
        Ok(PixiControl { tmpdir: tempdir })
    }

    /// Loads the project manifest and returns it.
    pub fn project(&self) -> anyhow::Result<Project> {
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
    pub async fn run(&self, mut args: run::Args) -> anyhow::Result<UnboundedReceiver<RunResult>> {
        args.manifest_path = args.manifest_path.or_else(|| Some(self.manifest_path()));
        let mut commands = order_commands(args.command, &self.project().unwrap())?;

        let project = self.project().unwrap();
        let command_env = get_command_env(&project).await.unwrap();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(command) = commands.pop_back() {
                let command = create_command(command, &project, &command_env)
                    .await
                    .expect("could not create command");
                if let Some(mut command) = command {
                    let tx = tx.clone();
                    spawn_blocking(move || {
                        let output = command
                            .stdout(Stdio::piped())
                            .spawn()
                            .expect("could not spawn task")
                            .wait_with_output()
                            .expect("could not run command");
                        tx.send(RunResult { output })
                            .expect("could not send output");
                    })
                    .await
                    .unwrap();
                }
            }
        });

        Ok(rx)
    }

    /// Create an installed environment. I.e a resolved and installed prefix
    pub async fn install(&self) -> anyhow::Result<()> {
        pixi::cli::install::execute(Args {
            manifest_path: Some(self.manifest_path()),
        })
        .await
    }

    /// Get the associated lock file
    pub async fn lock_file(&self) -> anyhow::Result<CondaLock> {
        pixi::environment::load_lock_for_manifest_path(&self.manifest_path()).await
    }
}

/// Contains the arguments to pass to `init::execute()`. Call `.await` to call the CLI execute
/// method and await the result at the same time.
pub struct InitBuilder {
    args: init::Args,
}

impl InitBuilder {
    pub fn with_channel(mut self, channel: impl ToString) -> Self {
        self.args.channels.push(channel.to_string());
        self
    }

    pub fn with_local_channel(self, channel: impl AsRef<Path>) -> Self {
        self.with_channel(Url::from_directory_path(channel).unwrap())
    }
}

// When `.await` is called on an object that is not a `Future` the compiler will first check if the
// type implements `IntoFuture`. If it does it will call the `IntoFuture::into_future()` method and
// await the resulting `Future`. We can abuse this behavior in builder patterns because the
// `into_future` method can also be used as a `finish` function. This allows you to reduce the
// required code.
impl IntoFuture for InitBuilder {
    type Output = anyhow::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(init::execute(self.args))
    }
}

/// Contains the arguments to pass to `add::execute()`. Call `.await` to call the CLI execute method
/// and await the result at the same time.
pub struct AddBuilder {
    args: add::Args,
}

impl AddBuilder {
    pub fn with_spec(mut self, spec: impl IntoMatchSpec) -> Self {
        self.args.specs.push(spec.into());
        self
    }

    /// Set as a host
    pub fn set_type(mut self, t: SpecType) -> Self {
        match t {
            SpecType::Host => {
                self.args.host = true;
                self.args.build = false;
            }
            SpecType::Build => {
                self.args.host = false;
                self.args.build = true;
            }
            SpecType::Run => {
                self.args.host = false;
                self.args.build = false;
            }
        }
        self
    }
}

// When `.await` is called on an object that is not a `Future` the compiler will first check if the
// type implements `IntoFuture`. If it does it will call the `IntoFuture::into_future()` method and
// await the resulting `Future`. We can abuse this behavior in builder patterns because the
// `into_future` method can also be used as a `finish` function. This allows you to reduce the
// required code.
impl IntoFuture for AddBuilder {
    type Output = anyhow::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(add::execute(self.args))
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
