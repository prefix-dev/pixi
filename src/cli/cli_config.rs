use crate::cli::has_specs::HasSpecs;
use crate::environment::LockFileUsage;
use crate::DependencyType;
use crate::Project;
use clap::Parser;
use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::{Config, ConfigCli};
use pixi_consts::consts;
use pixi_manifest::FeaturesExt;
use pixi_manifest::{FeatureName, SpecType};
use rattler_conda_types::ChannelConfig;
use rattler_conda_types::{Channel, NamedChannelOrUrl, Platform};
use std::collections::HashMap;
use std::path::PathBuf;

/// Project configuration
#[derive(Parser, Debug, Default)]
pub struct ProjectConfig {
    /// The path to `pixi.toml` or `pyproject.toml`
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

/// Channel configuration
#[derive(Parser, Debug, Default)]
pub struct ChannelsConfig {
    /// The channels to consider as a name or a url.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    ///
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(long = "channel", short = 'c', value_name = "CHANNEL")]
    channels: Vec<NamedChannelOrUrl>,
}

impl ChannelsConfig {
    /// Parses the channels, getting channel config and default channels from config
    pub(crate) fn resolve_from_config(&self, config: &Config) -> miette::Result<IndexSet<Channel>> {
        self.resolve(config.global_channel_config(), config.default_channels())
    }

    /// Parses the channels, getting channel config and default channels from project
    pub(crate) fn resolve_from_project(
        &self,
        project: Option<&Project>,
    ) -> miette::Result<IndexSet<Channel>> {
        match project {
            Some(project) => {
                let channels = project
                    .default_environment()
                    .channels()
                    .into_iter()
                    .cloned()
                    .collect_vec();
                self.resolve(&project.channel_config(), channels)
            }
            None => self.resolve_from_config(&Config::load_global()),
        }
    }

    /// Parses the channels from specified channel config and default channels
    fn resolve(
        &self,
        channel_config: &ChannelConfig,
        default_channels: Vec<NamedChannelOrUrl>,
    ) -> miette::Result<IndexSet<Channel>> {
        let channels = if self.channels.is_empty() {
            default_channels
        } else {
            self.channels.clone()
        };
        channels
            .into_iter()
            .map(|c| c.into_channel(channel_config))
            .try_collect()
            .into_diagnostic()
    }
}

/// Configuration for how to update the prefix
#[derive(Parser, Debug, Default, Clone)]
pub struct PrefixUpdateConfig {
    /// Don't update lockfile, implies the no-install as well.
    #[clap(long, conflicts_with = "no_install")]
    pub no_lockfile_update: bool,

    /// Lock file usage from the CLI
    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageArgs,

    /// Don't modify the environment, only modify the lock-file.
    #[arg(long)]
    pub no_install: bool,

    #[clap(flatten)]
    pub config: ConfigCli,
}
impl PrefixUpdateConfig {
    pub fn lock_file_usage(&self) -> LockFileUsage {
        if self.lock_file_usage.locked {
            LockFileUsage::Locked
        } else if self.lock_file_usage.frozen || self.no_lockfile_update {
            LockFileUsage::Frozen
        } else {
            LockFileUsage::Update
        }
    }

    /// Decide whether to install or not.
    pub(crate) fn no_install(&self) -> bool {
        self.no_install || self.no_lockfile_update
    }
}
#[derive(Parser, Debug, Default)]
pub struct DependencyConfig {
    /// The dependencies as names, conda MatchSpecs or PyPi requirements
    #[arg(required = true)]
    pub specs: Vec<String>,

    /// The specified dependencies are host dependencies. Conflicts with `build`
    /// and `pypi`
    #[arg(long, conflicts_with_all = ["build", "pypi"])]
    pub host: bool,

    /// The specified dependencies are build dependencies. Conflicts with `host`
    /// and `pypi`
    #[arg(long, conflicts_with_all = ["host", "pypi"])]
    pub build: bool,

    /// The specified dependencies are pypi dependencies. Conflicts with `host`
    /// and `build`
    #[arg(long, conflicts_with_all = ["host", "build"])]
    pub pypi: bool,

    /// The platform(s) for which the dependency should be modified
    #[arg(long, short)]
    pub platform: Vec<Platform>,

    /// The feature for which the dependency should be modified
    #[arg(long, short)]
    pub feature: Option<String>,
}

impl DependencyConfig {
    pub(crate) fn dependency_type(&self) -> DependencyType {
        if self.pypi {
            DependencyType::PypiDependency
        } else if self.host {
            DependencyType::CondaDependency(SpecType::Host)
        } else if self.build {
            DependencyType::CondaDependency(SpecType::Build)
        } else {
            DependencyType::CondaDependency(SpecType::Run)
        }
    }
    pub(crate) fn feature_name(&self) -> FeatureName {
        self.feature
            .clone()
            .map_or(FeatureName::Default, FeatureName::Named)
    }
    pub(crate) fn display_success(
        &self,
        operation: &str,
        implicit_constraints: HashMap<String, String>,
    ) {
        for package in self.specs.clone() {
            eprintln!(
                "{}{operation} {}{}",
                console::style(console::Emoji("✔ ", "")).green(),
                console::style(&package).bold(),
                if let Some(constraint) = implicit_constraints.get(&package) {
                    format!(" {}", console::style(constraint).dim())
                } else {
                    "".to_string()
                }
            );
        }

        // Print if it is something different from host and dep
        let dependency_type = self.dependency_type();
        if !matches!(
            dependency_type,
            DependencyType::CondaDependency(SpecType::Run)
        ) {
            eprintln!(
                "{operation} these as {}.",
                console::style(dependency_type.name()).bold()
            );
        }

        // Print something if we've modified for platforms
        if !self.platform.is_empty() {
            eprintln!(
                "{operation} these only for platform(s): {}",
                console::style(self.platform.iter().join(", ")).bold()
            )
        }
        // Print something if we've modified for features
        if let Some(feature) = &self.feature {
            {
                eprintln!(
                    "{operation} these only for feature: {}",
                    consts::FEATURE_STYLE.apply_to(feature)
                )
            }
        }
    }
}

impl HasSpecs for DependencyConfig {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}
