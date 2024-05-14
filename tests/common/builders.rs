//! Contains builders for the CLI commands
//! We are using a builder pattern here to make it easier to write tests.
//! And are kinda abusing the `IntoFuture` trait to make it easier to execute as close
//! as we can get to the command line args
//!
//! # Using IntoFuture
//!
//! When `.await` is called on an object that is not a `Future` the compiler will first check if the
//! type implements `IntoFuture`. If it does it will call the `IntoFuture::into_future()` method and
//! await the resulting `Future`. We can abuse this behavior in builder patterns because the
//! `into_future` method can also be used as a `finish` function. This allows you to reduce the
//! required code.
//!
//! ```rust
//! impl IntoFuture for InitBuilder {
//!     type Output = miette::Result<()>;
//!     type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;
//!
//!     fn into_future(self) -> Self::IntoFuture {
//!         Box::pin(init::execute(self.args))
//!     }
//! }
//!
//! ```

use futures::FutureExt;
use pixi::cli::remove;
use pixi::task::TaskName;
use pixi::{
    cli::{add, init, install, project, task},
    DependencyType, SpecType,
};
use rattler_conda_types::Platform;
use std::{
    future::{Future, IntoFuture},
    path::{Path, PathBuf},
    pin::Pin,
};
use url::Url;

/// Strings from an iterator
pub fn string_from_iter(iter: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    iter.into_iter().map(|s| s.as_ref().to_string()).collect()
}

/// Contains the arguments to pass to [`init::execute()`]. Call `.await` to call the CLI execute
/// method and await the result at the same time.
pub struct InitBuilder {
    pub args: init::Args,
}

impl InitBuilder {
    pub fn with_channel(mut self, channel: impl ToString) -> Self {
        self.args
            .channels
            .get_or_insert_with(Default::default)
            .push(channel.to_string());
        self
    }

    pub fn with_local_channel(self, channel: impl AsRef<Path>) -> Self {
        self.with_channel(Url::from_directory_path(channel).unwrap())
    }

    pub fn without_channels(mut self) -> Self {
        self.args.channels = Some(vec![]);
        self
    }
}

impl IntoFuture for InitBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        init::execute(self.args).boxed_local()
    }
}

/// Contains the arguments to pass to [`add::execute()`]. Call `.await` to call the CLI execute method
/// and await the result at the same time.
pub struct AddBuilder {
    pub args: add::Args,
}

impl AddBuilder {
    pub fn with_spec(mut self, spec: &str) -> Self {
        self.args.specs.push(spec.to_string());
        self
    }

    /// Set as a host
    pub fn set_type(mut self, t: DependencyType) -> Self {
        match t {
            DependencyType::CondaDependency(spec_type) => match spec_type {
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
            },
            DependencyType::PypiDependency => {
                self.args.host = false;
                self.args.build = false;
                self.args.pypi = true;
            }
        }
        self
    }

    /// Set whether to also install the environment. By default, the environment is NOT
    /// installed to reduce test times.
    pub fn with_install(mut self, install: bool) -> Self {
        self.args.no_install = !install;
        self
    }

    /// Skip updating lockfile, this will only check if it can add a dependencies.
    /// If it can add it will only add it to the manifest. Install will be skipped by default.
    pub fn without_lockfile_update(mut self) -> Self {
        self.args.no_lockfile_update = true;
        self
    }

    pub fn set_platforms(mut self, platforms: &[Platform]) -> Self {
        self.args.platform.extend(platforms.iter());
        self
    }

    pub fn set_editable(mut self, editable: bool) -> Self {
        self.args.editable = editable;
        self
    }
}

impl IntoFuture for AddBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        add::execute(self.args).boxed_local()
    }
}

/// Contains the arguments to pass to [`remove::execute()`]. Call `.await` to call the CLI execute method
/// and await the result at the same time.
pub struct RemoveBuilder {
    pub args: remove::Args,
}

impl RemoveBuilder {
    pub fn with_spec(mut self, spec: &str) -> Self {
        self.args.deps.push(spec.to_string());
        self
    }

    /// Set whether to also install the environment. By default, the environment is NOT
    /// installed to reduce test times.
    pub fn with_install(mut self, install: bool) -> Self {
        self.args.no_install = !install;
        self
    }

    /// Set as a host
    pub fn set_type(mut self, t: DependencyType) -> Self {
        match t {
            DependencyType::CondaDependency(spec_type) => match spec_type {
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
            },
            DependencyType::PypiDependency => {
                self.args.host = false;
                self.args.build = false;
                self.args.pypi = true;
            }
        }
        self
    }
}

impl IntoFuture for RemoveBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        remove::execute(self.args).boxed_local()
    }
}
pub struct TaskAddBuilder {
    pub manifest_path: Option<PathBuf>,
    pub args: task::AddArgs,
}

impl TaskAddBuilder {
    /// Execute these commands
    pub fn with_commands(mut self, commands: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        self.args.commands = string_from_iter(commands);
        self
    }

    /// Depends on these commands
    pub fn with_depends_on(mut self, depends: Vec<TaskName>) -> Self {
        self.args.depends_on = Some(depends);
        self
    }

    /// With this working directory
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.args.cwd = Some(cwd);
        self
    }

    /// With this environment variable
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.args.env = env;
        self
    }

    /// Execute the CLI command
    pub fn execute(self) -> miette::Result<()> {
        task::execute(task::Args {
            operation: task::Operation::Add(self.args),
            manifest_path: self.manifest_path,
        })
    }
}

pub struct TaskAliasBuilder {
    pub manifest_path: Option<PathBuf>,
    pub args: task::AliasArgs,
}

impl TaskAliasBuilder {
    /// Depends on these commands
    pub fn with_depends_on(mut self, depends: Vec<TaskName>) -> Self {
        self.args.depends_on = depends;
        self
    }

    /// Execute the CLI command
    pub fn execute(self) -> miette::Result<()> {
        task::execute(task::Args {
            operation: task::Operation::Alias(self.args),
            manifest_path: self.manifest_path,
        })
    }
}

pub struct ProjectChannelAddBuilder {
    pub manifest_path: Option<PathBuf>,
    pub args: project::channel::add::Args,
}

impl ProjectChannelAddBuilder {
    /// Adds the specified channel
    pub fn with_channel(mut self, name: impl Into<String>) -> Self {
        self.args.channel.push(name.into());
        self
    }

    /// Alias to add a local channel.
    pub fn with_local_channel(self, channel: impl AsRef<Path>) -> Self {
        self.with_channel(Url::from_directory_path(channel).unwrap())
    }
}

impl IntoFuture for ProjectChannelAddBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        project::channel::execute(project::channel::Args {
            manifest_path: self.manifest_path,
            command: project::channel::Command::Add(self.args),
        })
        .boxed_local()
    }
}

/// Contains the arguments to pass to [`install::execute()`]. Call `.await` to call the CLI execute method
/// and await the result at the same time.
pub struct InstallBuilder {
    pub args: install::Args,
}

impl InstallBuilder {
    pub fn with_locked(mut self) -> Self {
        self.args.lock_file_usage.locked = true;
        self
    }
    pub fn with_frozen(mut self) -> Self {
        self.args.lock_file_usage.frozen = true;
        self
    }
}

impl IntoFuture for InstallBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;
    fn into_future(self) -> Self::IntoFuture {
        install::execute(self.args).boxed_local()
    }
}
