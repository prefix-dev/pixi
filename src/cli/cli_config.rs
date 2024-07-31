use crate::cli::has_specs::HasSpecs;
use crate::environment::LockFileUsage;
use crate::DependencyType;
use clap::Parser;
use itertools::Itertools;
use pixi_config::ConfigCli;
use pixi_manifest::{FeatureName, SpecType};
use rattler_conda_types::Platform;
use std::collections::HashMap;
use std::path::PathBuf;

/// Project configuration
#[derive(Parser, Debug, Default)]
pub struct ProjectConfig {
    /// The path to 'pixi.toml' or 'pyproject.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

/// Configuration for how to update the prefix
#[derive(Parser, Debug, Default, Clone)]
pub struct PrefixUpdateConfig {
    /// Don't update lockfile, implies the no-install as well.
    #[clap(long, conflicts_with = "no_install")]
    pub no_lockfile_update: bool,

    /// Don't modify the environment, only modify the lock-file.
    #[arg(long)]
    pub no_install: bool,

    #[clap(flatten)]
    pub config: ConfigCli,
}
impl PrefixUpdateConfig {
    pub fn lock_file_usage(&self) -> LockFileUsage {
        if self.no_lockfile_update {
            LockFileUsage::Frozen
        } else {
            LockFileUsage::Update
        }
    }

    /// Decide whether to install or not.
    pub fn no_install(&self) -> bool {
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
    pub fn dependency_type(&self) -> DependencyType {
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
    pub fn feature_name(&self) -> FeatureName {
        self.feature
            .clone()
            .map_or(FeatureName::Default, FeatureName::Named)
    }
    pub fn display_success(&self, operation: &str, implicit_constraints: HashMap<String, String>) {
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
    }
}

impl HasSpecs for DependencyConfig {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}
