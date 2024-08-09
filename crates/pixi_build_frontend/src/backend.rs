use std::path::PathBuf;

use rattler_conda_types::MatchSpec;

use crate::BackendOverrides;

/// Describes the specification of the backend. This can be used to cache tool
/// information.
#[derive(Debug, Clone)]
pub enum BackendSpec {
    Isolated(IsolatedBackendSpec),
    System(SystemBackend),
}

/// A build tool that can be installed through a conda package.
#[derive(Debug, Clone)]
pub struct IsolatedBackendSpec {
    /// The specs used to instantiate the isolated build environment.
    pub specs: Vec<MatchSpec>,

    /// The command to invoke in the isolated environment.
    pub command: Option<String>,
}

impl IsolatedBackendSpec {
    /// Construct a new instance from a list of match specs.
    pub fn from_specs(specs: impl IntoIterator<Item = MatchSpec>) -> Self {
        Self {
            specs: specs.into_iter().collect(),
            command: None,
        }
    }

    /// Explicitly set the command to invoke.
    pub fn with_command(self, command: impl Into<String>) -> Self {
        Self {
            command: Some(command.into()),
            ..self
        }
    }
}

impl From<IsolatedBackendSpec> for BackendSpec {
    fn from(value: IsolatedBackendSpec) -> Self {
        Self::Isolated(value)
    }
}

/// A build tool that is installed on the system.
#[derive(Debug, Clone)]
pub struct SystemBackend {
    /// The command to invoke.
    pub command: PathBuf,
}

impl From<SystemBackend> for BackendSpec {
    fn from(value: SystemBackend) -> Self {
        Self::System(value)
    }
}

impl BackendOverrides {
    pub fn into_spec(self) -> Option<BackendSpec> {
        if let Some(spec) = self.spec {
            return Some(BackendSpec::Isolated(IsolatedBackendSpec::from_specs(
                vec![spec],
            )));
        }

        if let Some(path) = self.path {
            return Some(BackendSpec::System(SystemBackend { command: path }));
        }

        None
    }
}
