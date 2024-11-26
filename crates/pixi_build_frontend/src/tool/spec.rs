use miette::IntoDiagnostic;
use pixi_consts::consts::CACHED_BUILD_ENVS_DIR;
use pixi_manifest::BuildSystem;
use pixi_progress::wrap_in_progress;
use pixi_utils::{EnvironmentHash, PrefixGuard};
use rattler::{install::Installer, package_cache::PackageCache};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, NamedChannelOrUrl, Platform};
use rattler_shell::{
    activation::{ActivationVariables, Activator},
    shell::ShellEnum,
};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};

use crate::{BackendOverride, InProcessBackend};

use super::{IsolatedTool, ToolContext};

/// Describes the specification of the tool. This can be used to cache tool
/// information.
#[derive(Debug)]
pub enum ToolSpec {
    Isolated(IsolatedToolSpec),
    System(SystemToolSpec),
    Io(InProcessBackend),
}

/// A build tool that can be installed through a conda package.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct IsolatedToolSpec {
    /// The specs used to instantiate the isolated build environment.
    pub specs: Vec<MatchSpec>,

    /// The command to invoke in the isolated environment.
    pub command: String,

    /// Channels to use to fetch the build tools.
    pub channels: Vec<NamedChannelOrUrl>,
}

impl IsolatedToolSpec {
    /// Construct a new instance from a list of match specs.
    pub fn from_specs(specs: impl IntoIterator<Item = MatchSpec>) -> Self {
        Self {
            specs: specs.into_iter().collect(),
            command: String::new(),
            channels: Vec::new(),
        }
    }

    /// Construct a new instance from a build section
    pub fn from_build_section(build_section: &BuildSystem) -> Self {
        Self {
            specs: build_section.dependencies.clone(),
            command: build_section.build_backend.clone(),
            channels: build_section.channels.clone(),
        }
    }

    /// Explicitly set the command to invoke.
    pub fn with_command(self, command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            ..self
        }
    }

}

impl From<IsolatedToolSpec> for ToolSpec {
    fn from(value: IsolatedToolSpec) -> Self {
        Self::Isolated(value)
    }
}

/// A build tool that is installed on the system.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct SystemToolSpec {
    /// The command to invoke.
    pub command: String,
}

impl From<SystemToolSpec> for ToolSpec {
    fn from(value: SystemToolSpec) -> Self {
        Self::System(value)
    }
}

impl BackendOverride {
    pub fn into_spec(self) -> ToolSpec {
        match self {
            BackendOverride::Spec(spec) => {
                ToolSpec::Isolated(IsolatedToolSpec::from_specs(vec![spec]))
            }
            BackendOverride::System(command) => ToolSpec::System(SystemToolSpec { command }),
            BackendOverride::Io(process) => ToolSpec::Io(process),
        }
    }
}
