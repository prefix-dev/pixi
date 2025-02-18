use crate::cli::has_specs::HasSpecs;
use crate::environment::LockFileUsage;
use crate::lock_file::UpdateMode;
use crate::workspace::DiscoveryStart;
use crate::DependencyType;
use crate::Workspace;
use clap::Parser;
use indexmap::IndexMap;
use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pep508_rs::Requirement;
use pixi_config::{Config, ConfigCli};
use pixi_consts::consts;
use pixi_manifest::pypi::PyPiPackageName;
use pixi_manifest::FeaturesExt;
use pixi_manifest::{FeatureName, SpecType};
use pixi_spec::GitReference;
use rattler_conda_types::ChannelConfig;
use rattler_conda_types::{Channel, NamedChannelOrUrl, Platform};
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

/// Workspace configuration
#[derive(Parser, Debug, Default, Clone)]
pub struct WorkspaceConfig {
    /// The path to `pixi.toml`, `pyproject.toml`, or the project directory
    #[arg(long, global = true)]
    pub manifest_path: Option<PathBuf>,
}

impl WorkspaceConfig {
    /// Returns the start location when trying to discover a workspace.
    pub fn workspace_locator_start(&self) -> DiscoveryStart {
        match &self.manifest_path {
            Some(path) => DiscoveryStart::ExplicitManifest(path.clone()),
            None => DiscoveryStart::CurrentDir,
        }
    }
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
        project: Option<&Workspace>,
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

    /// Run the complete environment validation. This will reinstall a broken environment.
    #[arg(long)]
    pub revalidate: bool,
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

    /// Which `[UpdateMode]` to use
    pub(crate) fn update_mode(&self) -> UpdateMode {
        if self.revalidate {
            UpdateMode::Revalidate
        } else {
            UpdateMode::QuickValidate
        }
    }
}

#[derive(Parser, Debug, Default, Clone)]
pub struct GitRev {
    /// The git branch
    #[clap(long, requires = "git", conflicts_with_all = ["tag", "rev", "pypi"])]
    pub branch: Option<String>,

    /// The git tag
    #[clap(long, requires = "git", conflicts_with_all = ["branch", "rev", "pypi"])]
    pub tag: Option<String>,

    /// The git revision
    #[clap(long, requires = "git", conflicts_with_all = ["branch", "tag", "pypi"])]
    pub rev: Option<String>,
}

impl GitRev {
    /// Create a new `GitRev`
    pub fn new() -> Self {
        Default::default()
    }

    /// Set the branch
    pub fn with_branch(mut self, branch: String) -> GitRev {
        self.branch = Some(branch);
        self
    }

    /// Set the revision
    pub fn with_rev(mut self, rev: String) -> GitRev {
        self.rev = Some(rev);
        self
    }

    /// Set the tag
    pub fn with_tag(mut self, tag: String) -> GitRev {
        self.tag = Some(tag);
        self
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Some(branch) = &self.branch {
            Some(branch)
        } else if let Some(tag) = &self.tag {
            Some(tag)
        } else if let Some(rev) = &self.rev {
            Some(rev)
        } else {
            None
        }
    }
}

impl From<GitRev> for GitReference {
    fn from(git_rev: GitRev) -> Self {
        if let Some(branch) = git_rev.branch {
            GitReference::Branch(branch)
        } else if let Some(tag) = git_rev.tag {
            GitReference::Tag(tag)
        } else if let Some(rev) = git_rev.rev {
            GitReference::Rev(rev)
        } else {
            GitReference::DefaultBranch
        }
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
    #[arg(long = "platform", short)]
    pub platforms: Vec<Platform>,

    /// The feature for which the dependency should be modified
    #[clap(long, short, default_value_t)]
    pub feature: FeatureName,

    /// The git url to use when adding a git dependency
    #[clap(long, short)]
    pub git: Option<Url>,

    #[clap(flatten)]
    /// The git revisions to use when adding a git dependency
    pub rev: Option<GitRev>,

    /// The subdirectory of the git repository to use
    #[clap(long, short, requires = "git")]
    pub subdir: Option<String>,
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
        if !self.platforms.is_empty() {
            eprintln!(
                "{operation} these only for platform(s): {}",
                console::style(self.platforms.iter().join(", ")).bold()
            )
        }
        // Print something if we've modified for features
        if let FeatureName::Named(feature) = &self.feature {
            {
                eprintln!(
                    "{operation} these only for feature: {}",
                    consts::FEATURE_STYLE.apply_to(feature)
                )
            }
        }
    }

    pub fn vcs_pep508_requirements(
        &self,
        project: &Workspace,
    ) -> Option<miette::Result<IndexMap<PyPiPackageName, Requirement>>> {
        match &self.git {
            Some(git) => {
                // pep 508 requirements with direct reference
                // should be in this format
                // name @ url@rev#subdirectory=subdir
                // we need to construct it
                let pep_reqs: miette::Result<IndexMap<PyPiPackageName, Requirement>> = self
                    .specs
                    .iter()
                    .map(|package_name| {
                        let vcs_req = build_vcs_requirement(
                            package_name,
                            git,
                            self.rev.as_ref(),
                            self.subdir.clone(),
                        );

                        let dep = Requirement::parse(&vcs_req, project.root()).into_diagnostic()?;
                        let name = PyPiPackageName::from_normalized(dep.clone().name);

                        Ok((name, dep))
                    })
                    .collect();
                Some(pep_reqs)
            }
            None => None,
        }
    }
}

impl HasSpecs for DependencyConfig {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

/// Builds a PEP 508 compliant VCS requirement string
fn build_vcs_requirement(
    package_name: &str,
    git: &Url,
    rev: Option<&GitRev>,
    subdir: Option<String>,
) -> String {
    let mut vcs_req = format!("{} @ {}", package_name, git);
    if let Some(rev_str) = rev.and_then(|rev| rev.as_str().map(|s| s.to_string())) {
        vcs_req.push_str(&format!("@{}", rev_str));
    }
    if let Some(subdir) = subdir {
        vcs_req.push_str(&format!("#subdirectory={}", subdir));
    }
    vcs_req
}

#[cfg(test)]
mod tests {
    use url::Url;

    use crate::cli::cli_config::{build_vcs_requirement, GitRev};

    #[test]
    fn test_build_vcs_requirement_with_all_fields() {
        let result = build_vcs_requirement(
            "mypackage",
            &Url::parse("https://github.com/user/repo").unwrap(),
            Some(&GitRev::new().with_tag("v1.0.0".to_string())),
            Some("subdir".to_string()),
        );
        assert_eq!(
            result,
            "mypackage @ https://github.com/user/repo@v1.0.0#subdirectory=subdir"
        );
    }

    #[test]
    fn test_build_vcs_requirement_with_no_rev() {
        let result = build_vcs_requirement(
            "mypackage",
            &Url::parse("https://github.com/user/repo").unwrap(),
            None,
            Some("subdir".to_string()),
        );
        assert_eq!(
            result,
            "mypackage @ https://github.com/user/repo#subdirectory=subdir"
        );
    }

    #[test]
    fn test_build_vcs_requirement_with_no_subdir() {
        let result = build_vcs_requirement(
            "mypackage",
            &Url::parse("https://github.com/user/repo").unwrap(),
            Some(&GitRev::new().with_tag("v1.0.0".to_string())),
            None,
        );
        assert_eq!(result, "mypackage @ https://github.com/user/repo@v1.0.0");
    }

    #[test]
    fn test_build_vcs_requirement_with_only_git() {
        let result = build_vcs_requirement(
            "mypackage",
            &Url::parse("https://github.com/user/repo").unwrap(),
            None,
            None,
        );
        assert_eq!(result, "mypackage @ https://github.com/user/repo");
    }
}
