use std::str::FromStr;

use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{Channel, ChannelConfig, ChannelUrl, NamelessMatchSpec};
use url::Url;

/// Describes how a backend should be instantiated.
#[derive(Debug)]
pub enum BackendSpec {
    /// Describes a backend that uses JSON-RPC to communicate with a backend.
    JsonRpc(JsonRpcBackendSpec),
    // TODO: Support in-memory backends without going through JSON-RPC.
}

/// Describes a backend that uses JSON-RPC to communicate with an executable.
#[derive(Debug)]
pub struct JsonRpcBackendSpec {
    /// The name of the backend
    pub name: String,

    /// The specification on how to instantiate the backend.
    pub command: CommandSpec,
}

/// Describes a command that should be run by calling an executable in a certain
/// environment.
#[derive(Debug)]
pub enum CommandSpec {
    EnvironmentSpec(EnvironmentSpec),
    System(SystemCommandSpec),
}

/// Describes a command that should be run by calling an executable on the
/// system.
#[derive(Debug, Default)]
pub struct SystemCommandSpec {
    /// The command to run. If this is `None` the command should be inferred
    /// from the name of the backend.
    pub command: Option<String>,
}

/// Describes a conda environment that should be set up in which the backend is
/// run.
#[derive(Debug)]
pub struct EnvironmentSpec {
    /// The main requirement
    pub requirement: (rattler_conda_types::PackageName, NamelessMatchSpec),

    /// The requirements for the environment.
    pub additional_requirements: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// Additional constraints to apply to the environment
    pub constraints: DependencyMap<rattler_conda_types::PackageName, NamelessMatchSpec>,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// The name of the command to invoke in the environment. If not specified,
    /// this should be derived from the name of the backend.
    pub command: Option<String>,
}

impl JsonRpcBackendSpec {
    /// Constructs a new default instance for spawning a recipe build backend.
    pub fn default_rattler_build(channel_config: &ChannelConfig) -> Self {
        const DEFAULT_BUILD_TOOL: &str = "pixi-build-rattler-build";

        let conda_forge_channel = Channel::from_name("conda-forge", channel_config).base_url;
        let backends_channel = Url::from_str("https://prefix.dev/pixi-build-backends")
            .unwrap()
            .into();

        Self {
            name: DEFAULT_BUILD_TOOL.to_string(),
            command: CommandSpec::EnvironmentSpec(EnvironmentSpec {
                requirement: (
                    DEFAULT_BUILD_TOOL.parse().unwrap(),
                    NamelessMatchSpec::default(),
                ),
                additional_requirements: Default::default(),
                constraints: Default::default(),
                channels: vec![conda_forge_channel, backends_channel],
                command: None,
            }),
        }
    }
}
