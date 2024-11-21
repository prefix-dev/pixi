use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_consts::consts::CACHED_BUILD_ENVS_DIR;
use pixi_manifest::BuildSection;
use pixi_utils::EnvironmentHash;
use rattler::{install::Installer, package_cache::PackageCache};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, Platform};
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
}

impl IsolatedToolSpec {
    /// Construct a new instance from a list of match specs.
    pub fn from_specs(specs: impl IntoIterator<Item = MatchSpec>) -> Self {
        Self {
            specs: specs.into_iter().collect(),
            command: String::new(),
        }
    }

    /// Construct a new instance from a build section
    pub fn from_build_section(build_section: &BuildSection) -> Self {
        Self {
            specs: build_section.dependencies.clone(),
            command: build_section.build_backend.clone(),
        }
    }

    /// Explicitly set the command to invoke.
    pub fn with_command(self, command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            ..self
        }
    }

    /// Installed the tool in the isolated environment.
    pub async fn install(&self, context: ToolContext) -> miette::Result<IsolatedTool> {
        let repodata = context
            .gateway
            .query(
                context.channels.clone(),
                [Platform::current(), Platform::NoArch],
                self.specs.clone(),
            )
            .recursive(true)
            .execute()
            .await
            .into_diagnostic()?;

        // Determine virtual packages of the current platform
        let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::from_env())
            .unwrap()
            .iter()
            .cloned()
            .map(GenericVirtualPackage::from)
            .collect();

        let solved_records = Solver
            .solve(SolverTask {
                specs: self.specs.clone(),
                virtual_packages,
                ..SolverTask::from_iter(&repodata)
            })
            .into_diagnostic()?;

        eprintln!("spec is {:?}", self.specs);
        if solved_records.is_empty() {
            miette::bail!(
                "could not find {}",
                self.specs.iter().map(|spec| spec.to_string()).join(",")
            );
        }

        let cache = EnvironmentHash::new(
            self.command.clone(),
            self.specs.clone(),
            context
                .channels
                .iter()
                .map(|c| c.base_url.to_string())
                .collect(),
        );

        let cached_dir = context
            .cache_dir
            .join(CACHED_BUILD_ENVS_DIR)
            .join(cache.name());

        // Install the environment
        Installer::new()
            .with_download_client(context.client.clone())
            .with_package_cache(PackageCache::new(
                context
                    .cache_dir
                    .join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
            ))
            .install(&cached_dir, solved_records)
            .await
            .into_diagnostic()?;

        // Get the activation scripts
        let activator =
            Activator::from_path(&cached_dir, ShellEnum::default(), Platform::current()).unwrap();

        let activation_scripts = activator
            .run_activation(ActivationVariables::from_env().unwrap_or_default(), None)
            .unwrap();

        Ok(IsolatedTool::new(
            self.command.clone(),
            cached_dir,
            activation_scripts,
        ))
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
