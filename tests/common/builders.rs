//! Contains builders for the CLI commands
//! We are using a builder pattern here to make it easier to write tests.
//! And are kinda abusing the `IntoFuture` trait to make it easier to execute as
//! close as we can get to the command line args
//!
//! # Using IntoFuture
//!
//! When `.await` is called on an object that is not a `Future` the compiler
//! will first check if the type implements `IntoFuture`. If it does it will
//! call the `IntoFuture::into_future()` method and await the resulting
//! `Future`. We can abuse this behavior in builder patterns because the
//! `into_future` method can also be used as a `finish` function. This allows
//! you to reduce the required code.
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
//! ```

use pixi::cli::cli_config::{PrefixUpdateConfig, ProjectConfig};
use std::{
    future::{Future, IntoFuture},
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
};

use futures::FutureExt;
use pixi::{
    cli::{add, cli_config::DependencyConfig, init, install, project, remove, task, update},
    task::TaskName,
    DependencyType,
};
use pixi_manifest::{EnvironmentName, SpecType};
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use url::Url;

/// Strings from an iterator
pub fn string_from_iter(iter: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    iter.into_iter().map(|s| s.as_ref().to_string()).collect()
}

/// Contains the arguments to pass to [`init::execute()`]. Call `.await` to call
/// the CLI execute method and await the result at the same time.
pub struct InitBuilder {
    pub args: init::Args,
    pub no_fast_prefix: bool,
}

impl InitBuilder {
    /// Disable using `https://fast.prefix.dev` as the default channel.
    pub fn no_fast_prefix_overwrite(self, no_fast_prefix: bool) -> Self {
        Self {
            no_fast_prefix,
            ..self
        }
    }

    pub fn with_channel(mut self, channel: impl ToString) -> Self {
        self.args
            .channels
            .get_or_insert_with(Default::default)
            .push(NamedChannelOrUrl::from_str(channel.to_string().as_str()).unwrap());
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
        init::execute(init::Args {
            channels: if !self.no_fast_prefix {
                self.args.channels.or_else(|| {
                    Some(vec![NamedChannelOrUrl::from_str(
                        "https://fast.prefix.dev/conda-forge",
                    )
                    .unwrap()])
                })
            } else {
                self.args.channels
            },
            ..self.args
        })
        .boxed_local()
    }
}
/// A trait used by AddBuilder and RemoveBuilder to set their inner
/// DependencyConfig
pub trait HasPrefixUpdateConfig: Sized {
    fn prefix_update_config(&mut self) -> &mut PrefixUpdateConfig;
    /// Set whether to also install the environment. By default, the environment
    /// is NOT installed to reduce test times.
    fn with_install(mut self, install: bool) -> Self {
        self.prefix_update_config().no_install = !install;
        self
    }

    /// Skip updating lockfile, this will only check if it can add a
    /// dependencies. If it can add it will only add it to the manifest.
    /// Install will be skipped by default.
    fn without_lockfile_update(mut self) -> Self {
        self.prefix_update_config().no_lockfile_update = true;
        self
    }
}

/// A trait used by AddBuilder and RemoveBuilder to set their inner
/// DependencyConfig
pub trait HasDependencyConfig: Sized {
    fn dependency_config(&mut self) -> &mut DependencyConfig;

    fn dependency_config_with_specs(specs: Vec<&str>) -> DependencyConfig {
        DependencyConfig {
            specs: specs.iter().map(|s| s.to_string()).collect(),
            host: false,
            build: false,
            pypi: false,
            platform: Default::default(),
            feature: None,
        }
    }

    fn with_spec(mut self, spec: &str) -> Self {
        self.dependency_config().specs.push(spec.to_string());
        self
    }

    /// Set as a host
    fn set_type(mut self, t: DependencyType) -> Self {
        match t {
            DependencyType::CondaDependency(spec_type) => match spec_type {
                SpecType::Host => {
                    self.dependency_config().host = true;
                    self.dependency_config().build = false;
                }
                SpecType::Build => {
                    self.dependency_config().host = false;
                    self.dependency_config().build = true;
                }
                SpecType::Run => {
                    self.dependency_config().host = false;
                    self.dependency_config().build = false;
                }
            },
            DependencyType::PypiDependency => {
                self.dependency_config().host = false;
                self.dependency_config().build = false;
                self.dependency_config().pypi = true;
            }
        }
        self
    }

    fn set_platforms(mut self, platforms: &[Platform]) -> Self {
        self.dependency_config().platform.extend(platforms.iter());
        self
    }
}

/// Contains the arguments to pass to [`add::execute()`]. Call `.await` to call
/// the CLI execute method and await the result at the same time.
pub struct AddBuilder {
    pub args: add::Args,
}

impl AddBuilder {
    pub fn set_editable(mut self, editable: bool) -> Self {
        self.args.editable = editable;
        self
    }

    pub fn with_feature(mut self, feature: impl ToString) -> Self {
        self.args.dependency_config.feature = Some(feature.to_string());
        self
    }
}

impl HasDependencyConfig for AddBuilder {
    fn dependency_config(&mut self) -> &mut DependencyConfig {
        &mut self.args.dependency_config
    }
}

impl HasPrefixUpdateConfig for AddBuilder {
    fn prefix_update_config(&mut self) -> &mut PrefixUpdateConfig {
        &mut self.args.prefix_update_config
    }
}

impl IntoFuture for AddBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;

    fn into_future(self) -> Self::IntoFuture {
        add::execute(self.args).boxed_local()
    }
}

/// Contains the arguments to pass to [`remove::execute()`]. Call `.await` to
/// call the CLI execute method and await the result at the same time.
pub struct RemoveBuilder {
    pub args: remove::Args,
}

impl HasDependencyConfig for RemoveBuilder {
    fn dependency_config(&mut self) -> &mut DependencyConfig {
        &mut self.args.dependency_config
    }
}

impl HasPrefixUpdateConfig for RemoveBuilder {
    fn prefix_update_config(&mut self) -> &mut PrefixUpdateConfig {
        &mut self.args.prefix_update_config
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
            project_config: ProjectConfig {
                manifest_path: self.manifest_path,
            },
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
            project_config: ProjectConfig {
                manifest_path: self.manifest_path,
            },
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
        self.args
            .channel
            .push(NamedChannelOrUrl::from_str(&name.into()).unwrap());
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

/// Contains the arguments to pass to [`install::execute()`]. Call `.await` to
/// call the CLI execute method and await the result at the same time.
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

pub struct ProjectEnvironmentAddBuilder {
    pub args: project::environment::add::Args,
    pub manifest_path: Option<PathBuf>,
}

impl ProjectEnvironmentAddBuilder {
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.args
            .features
            .get_or_insert_with(Vec::new)
            .push(feature.into());
        self
    }

    pub fn with_no_default_features(mut self, no_default_features: bool) -> Self {
        self.args.no_default_feature = no_default_features;
        self
    }

    pub fn force(mut self, force: bool) -> Self {
        self.args.force = force;
        self
    }

    pub fn with_solve_group(mut self, solve_group: impl Into<String>) -> Self {
        self.args.solve_group = Some(solve_group.into());
        self
    }
}

impl IntoFuture for ProjectEnvironmentAddBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;
    fn into_future(self) -> Self::IntoFuture {
        project::environment::execute(project::environment::Args {
            manifest_path: self.manifest_path,
            command: project::environment::Command::Add(self.args),
        })
        .boxed_local()
    }
}

/// Contains the arguments to pass to [`update::exeecute()`]. Call `.await` to
/// call the CLI execute method and await the result at the same time.
pub struct UpdateBuilder {
    pub args: update::Args,
}

impl UpdateBuilder {
    pub fn with_package(mut self, package: impl ToString) -> Self {
        self.args
            .specs
            .packages
            .get_or_insert_with(Vec::new)
            .push(package.to_string());
        self
    }

    pub fn with_environment(mut self, env: impl Into<EnvironmentName>) -> Self {
        self.args
            .specs
            .environments
            .get_or_insert_with(Vec::new)
            .push(env.into());
        self
    }

    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.args
            .specs
            .platforms
            .get_or_insert_with(Vec::new)
            .push(platform);
        self
    }

    pub fn dry_run(mut self, dry_run: bool) -> Self {
        self.args.dry_run = dry_run;
        self
    }

    pub fn json(mut self, json: bool) -> Self {
        self.args.json = json;
        self
    }
}

impl IntoFuture for UpdateBuilder {
    type Output = miette::Result<()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'static>>;
    fn into_future(self) -> Self::IntoFuture {
        update::execute(self.args).boxed_local()
    }
}
