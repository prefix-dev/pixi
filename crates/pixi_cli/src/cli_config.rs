use crate::has_specs::HasSpecs;
use clap::Parser;
use indexmap::IndexMap;
use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pep508_rs::Requirement;
use pixi_build_frontend::BackendOverride;
use pixi_config::Config;
use pixi_consts::consts;
use pixi_core::DependencyType;
use pixi_core::Workspace;
use pixi_core::environment::LockFileUsage;
use pixi_core::workspace::DiscoveryStart;
use pixi_manifest::FeaturesExt;
use pixi_manifest::{FeatureName, SpecType};
use pixi_spec::GitReference;
use rattler_conda_types::ChannelConfig;
use rattler_conda_types::{Channel, NamedChannelOrUrl, Platform};
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

use pixi_git::GIT_URL_QUERY_REV_TYPE;
use pixi_pypi_spec::PypiPackageName;

/// Workspace configuration
#[derive(Parser, Debug, Default, Clone)]
pub struct WorkspaceConfig {
    /// The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
    #[arg(long, short, global = true, conflicts_with = "name", help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    pub manifest_path: Option<PathBuf>,

    /// Backend override for testing purposes. This field is ignored by clap
    /// and should only be set programmatically in tests.
    #[clap(skip)]
    pub backend_override: Option<BackendOverride>,

    /// Name of the workspace
    #[arg(long, global = true, conflicts_with = "manifest_path", help_heading = consts::CLAP_GLOBAL_OPTIONS)]
    pub name: Option<String>,
}

impl WorkspaceConfig {
    /// Returns the start location when trying to discover a workspace.
    pub fn workspace_locator_start(&self) -> DiscoveryStart {
        if let Some(manifest_path) = &self.manifest_path {
            DiscoveryStart::ExplicitManifest(manifest_path.clone())
        } else if let Some(name) = &self.name {
            let config = Config::load_global();
            let path = config.named_workspace(&name.to_string()).unwrap();
            DiscoveryStart::ExplicitManifest(path.clone())
        } else {
            DiscoveryStart::CurrentDir
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

#[derive(Parser, Debug, Default, Clone)]
pub struct LockFileUpdateConfig {
    /// DEPRECATED: use `--frozen` `--no-install`. Skips lock-file updates
    #[clap(hide = true, long, help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub no_lockfile_update: bool,

    /// Lock file usage from the CLI
    #[clap(flatten)]
    pub lock_file_usage: super::LockFileUsageConfig,
}

impl LockFileUpdateConfig {
    pub fn lock_file_usage(&self) -> miette::Result<LockFileUsage> {
        // Error on deprecated flag usage
        if self.no_lockfile_update {
            return Err(miette::miette!(
                help = "Use '--frozen' to skip lock-file updates.\nUse '--no-install' to skip installation.",
                "The '--no-lockfile-update' flag has been deprecated due to inconsistent behavior across commands. This flag will be removed in a future version."
            ));
        }

        Ok(self.lock_file_usage.to_usage())
    }
}

/// Configuration for skipping installation
#[derive(Parser, Debug, Default, Clone)]
pub struct NoInstallConfig {
    /// Don't modify the environment, only modify the lock-file.
    #[arg(long, help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub no_install: bool,
}

impl NoInstallConfig {
    /// Creates a new NoInstallConfig with the specified value
    pub fn new(no_install: bool) -> Self {
        Self { no_install }
    }

    pub fn allow_installs(&self) -> bool {
        !self.no_install
    }
}

/// Lock file and installation configuration with --as-is support
/// Used by shell, shell-hook, and run commands
#[derive(Parser, Debug, Default, Clone)]
pub struct LockAndInstallConfig {
    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    /// Shorthand for the combination of --no-install and --frozen.
    #[arg(long, help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub as_is: bool,
}

impl LockAndInstallConfig {
    /// Returns true if the --as-is flag is set or if the no_install flag is set
    pub fn no_install(&self) -> bool {
        self.as_is || self.no_install_config.no_install
    }

    /// Get the effective lock file usage based on the configuration
    pub fn lock_file_usage(&self) -> miette::Result<pixi_core::environment::LockFileUsage> {
        // If --as-is is set this is equivalent to --frozen and --no-install
        if self.as_is {
            return Ok(LockFileUsage::Frozen);
        }

        // Otherwise use the normal lock file update config
        self.lock_file_update_config.lock_file_usage()
    }

    /// Check if installs are allowed (considering --as-is)
    pub fn allow_installs(&self) -> bool {
        !self.as_is && self.no_install_config.allow_installs()
    }
}

#[derive(Parser, Debug, Default, Clone)]
pub struct GitRev {
    /// The git branch
    #[clap(long, requires = "git", conflicts_with_all = ["tag", "rev"], help_heading = consts::CLAP_GIT_OPTIONS)]
    pub branch: Option<String>,

    /// The git tag
    #[clap(long, requires = "git", conflicts_with_all = ["branch", "rev"], help_heading = consts::CLAP_GIT_OPTIONS)]
    pub tag: Option<String>,

    /// The git revision
    #[clap(long, requires = "git", conflicts_with_all = ["branch", "tag"], help_heading = consts::CLAP_GIT_OPTIONS)]
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

    /// Get the reference as a string
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

    /// Get the type of the reference
    pub fn reference_type(&self) -> Option<&str> {
        if self.branch.is_some() {
            Some("branch")
        } else if self.tag.is_some() {
            Some("tag")
        } else if self.rev.is_some() {
            Some("rev")
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
    /// The dependency as names, conda MatchSpecs or PyPi requirements
    #[arg(required = true, value_name = "SPEC")]
    pub specs: Vec<String>,

    /// The specified dependencies are host dependencies. Conflicts with `build`
    /// and `pypi`
    #[arg(long, conflicts_with_all = ["build", "pypi"], hide = true)]
    pub host: bool,

    /// The specified dependencies are build dependencies. Conflicts with `host`
    /// and `pypi`
    #[arg(long, conflicts_with_all = ["host", "pypi"], hide = true)]
    pub build: bool,

    /// The specified dependencies are pypi dependencies. Conflicts with `host`
    /// and `build`
    #[arg(long, conflicts_with_all = ["host", "build"])]
    pub pypi: bool,

    /// The platform for which the dependency should be modified.
    #[arg(long = "platform", short, value_name = "PLATFORM")]
    pub platforms: Vec<Platform>,

    /// The feature for which the dependency should be modified.
    #[clap(long, short, default_value_t)]
    pub feature: FeatureName,

    /// The git url to use when adding a git dependency
    #[clap(long, short, help_heading = consts::CLAP_GIT_OPTIONS)]
    pub git: Option<Url>,

    #[clap(flatten)]
    /// The git revisions to use when adding a git dependency
    pub rev: Option<GitRev>,

    /// The subdirectory of the git repository to use
    #[clap(long, short, requires = "git", help_heading = consts::CLAP_GIT_OPTIONS)]
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
        if let Some(feature) = self.feature.non_default() {
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
    ) -> Option<miette::Result<IndexMap<PypiPackageName, Requirement>>> {
        match &self.git {
            Some(git) => {
                // pep 508 requirements with direct reference
                // should be in this format
                // name @ url@rev#subdirectory=subdir
                // we need to construct it
                let pep_reqs: miette::Result<IndexMap<PypiPackageName, Requirement>> = self
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
                        let name = PypiPackageName::from_normalized(dep.clone().name);

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

/// Builds a PEP 508 compliant VCS requirement string.
/// Main difference between a simple VCS requirement is that it encode
/// in a separate query parameter the reference type.
/// This is used to differentiate between a branch, a tag or a revision
/// which is lost in the simple VCS requirement.
/// Return a string in the format `name @ git+url@rev?rev_type=type#subdirectory=subdir`
/// where `rev_type` is added only if reference is present.
fn build_vcs_requirement(
    package_name: &str,
    git: &Url,
    rev: Option<&GitRev>,
    subdir: Option<String>,
) -> String {
    let scheme = if git.scheme().starts_with("git+") {
        ""
    } else {
        "git+"
    };
    let mut vcs_req = format!("{package_name} @ {scheme}{git}");
    if let Some(revision) = rev
        && let Some(rev_str) = revision.as_str().map(|s| s.to_string())
    {
        vcs_req.push_str(&format!("@{rev_str}"));

        if let Some(rev_type) = revision.reference_type() {
            vcs_req.push_str(&format!("?{GIT_URL_QUERY_REV_TYPE}={rev_type}"));
        }
    }
    if let Some(subdir) = subdir {
        vcs_req.push_str(&format!("#subdirectory={subdir}"));
    }

    vcs_req
}

#[cfg(test)]
mod tests {
    use url::Url;

    use crate::cli_config::{
        GitRev, LockAndInstallConfig, LockFileUpdateConfig, NoInstallConfig, build_vcs_requirement,
    };
    use pixi_core::environment::LockFileUsage;

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
            "mypackage @ git+https://github.com/user/repo@v1.0.0?rev_type=tag#subdirectory=subdir"
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
            "mypackage @ git+https://github.com/user/repo#subdirectory=subdir"
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
        assert_eq!(
            result,
            "mypackage @ git+https://github.com/user/repo@v1.0.0?rev_type=tag"
        );
    }

    #[test]
    fn test_build_vcs_requirement_with_only_git() {
        let result = build_vcs_requirement(
            "mypackage",
            &Url::parse("https://github.com/user/repo").unwrap(),
            None,
            None,
        );
        assert_eq!(result, "mypackage @ git+https://github.com/user/repo");
    }

    #[test]
    fn test_build_vcs_requirement_with_local_dir() {
        let result = build_vcs_requirement(
            "mypackage",
            &Url::parse("file:///home/user/GitHub/mypackage").unwrap(),
            None,
            None,
        );
        assert_eq!(result, "mypackage @ git+file:///home/user/GitHub/mypackage");
    }

    #[test]
    fn test_lock_and_install_config_as_is_flag() {
        // Test --as-is sets both frozen and no_install
        let config = LockAndInstallConfig {
            as_is: true,
            no_install_config: NoInstallConfig::default(),
            lock_file_update_config: LockFileUpdateConfig::default(),
        };

        assert!(config.no_install(), "as_is should enable no_install");
        assert!(!config.allow_installs(), "as_is should disable installs");

        let lock_usage = config.lock_file_usage().unwrap();
        assert!(
            matches!(lock_usage, LockFileUsage::Frozen),
            "as_is should set lock file usage to Frozen"
        );
    }

    #[test]
    fn test_lock_and_install_config_respects_individual_flags() {
        // Test that individual flags still work when --as-is is not set
        let config = LockAndInstallConfig {
            as_is: false,
            no_install_config: NoInstallConfig::new(true),
            lock_file_update_config: {
                let mut lock_config = LockFileUpdateConfig::default();
                lock_config.lock_file_usage.frozen = true;
                lock_config
            },
        };

        assert!(
            config.no_install(),
            "should respect individual no_install flag"
        );
        assert!(
            !config.allow_installs(),
            "should respect individual no_install flag"
        );

        let lock_usage = config.lock_file_usage().unwrap();
        assert!(
            matches!(lock_usage, LockFileUsage::Frozen),
            "should respect individual frozen flag"
        );
    }
}
