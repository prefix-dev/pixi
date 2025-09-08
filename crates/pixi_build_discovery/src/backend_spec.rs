use std::str::FromStr;

use pixi_spec::{BinarySpec, PixiSpec, SourceAnchor};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{Channel, ChannelConfig, ChannelUrl};
use url::Url;

/// Describes how a backend should be instantiated.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "kebab-case"))]
pub enum BackendSpec {
    /// Describes a backend that uses JSON-RPC to communicate with a backend.
    JsonRpc(JsonRpcBackendSpec),
    // TODO: Support in-memory backends without going through JSON-RPC.
}

impl BackendSpec {
    /// Resolves the backend specification to a relative path based on the
    /// provided source anchor.
    pub fn resolve(self, source_anchor: SourceAnchor) -> Self {
        match self {
            BackendSpec::JsonRpc(spec) => BackendSpec::JsonRpc(spec.resolve(source_anchor)),
        }
    }
}

/// Describes a backend that uses JSON-RPC to communicate with an executable.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub struct JsonRpcBackendSpec {
    /// The name of the backend
    pub name: String,

    /// The specification on how to instantiate the backend.
    pub command: CommandSpec,
}

impl JsonRpcBackendSpec {
    /// Resolves the JsonRpcBackendSpec to a relative path based on the
    /// provided source anchor.
    pub fn resolve(self, source_anchor: SourceAnchor) -> Self {
        Self {
            name: self.name,
            command: {
                match self.command {
                    CommandSpec::EnvironmentSpec(mut env_spec) => {
                        let maybe_source_spec = env_spec.requirement.1.try_into_source_spec();
                        let pixi_spec = match maybe_source_spec {
                            Ok(source_spec) => {
                                let resolved_spec = source_anchor.resolve(source_spec);
                                PixiSpec::from(resolved_spec)
                            }
                            Err(pixi_spec) => pixi_spec,
                        };
                        env_spec.requirement.1 = pixi_spec;
                        CommandSpec::EnvironmentSpec(env_spec)
                    }
                    CommandSpec::System(system_spec) => CommandSpec::System(system_spec),
                }
            },
        }
    }
}

/// Describes a command that should be run by calling an executable in a certain
/// environment.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "kebab-case"))]
pub enum CommandSpec {
    EnvironmentSpec(Box<EnvironmentSpec>),
    System(SystemCommandSpec),
}

impl CommandSpec {
    /// Resolves the CommandSpec to a relative path based on the
    /// provided source anchor.
    pub fn resolve(self, source_anchor: SourceAnchor) -> Self {
        match self {
            CommandSpec::EnvironmentSpec(env_spec) => {
                CommandSpec::EnvironmentSpec(Box::new(env_spec.resolve(source_anchor)))
            }
            CommandSpec::System(system_spec) => CommandSpec::System(system_spec),
        }
    }
}

/// Describes a command that should be run by calling an executable on the
/// system.
#[derive(Debug, Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub struct SystemCommandSpec {
    /// The command to run. If this is `None` the command should be inferred
    /// from the name of the backend.
    pub command: Option<String>,
}

/// Describes a conda environment that should be set up in which the backend is
/// run.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub struct EnvironmentSpec {
    /// The main requirement
    pub requirement: (rattler_conda_types::PackageName, PixiSpec),

    /// The requirements for the environment.
    #[cfg_attr(
        feature = "serde",
        serde(skip_serializing_if = "DependencyMap::is_empty")
    )]
    pub additional_requirements: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,

    /// Additional constraints to apply to the environment
    #[cfg_attr(
        feature = "serde",
        serde(skip_serializing_if = "DependencyMap::is_empty")
    )]
    pub constraints: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,

    /// The channels to use for solving
    pub channels: Vec<ChannelUrl>,

    /// The name of the command to invoke in the environment. If not specified,
    /// this should be derived from the name of the backend.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub command: Option<String>,
}

impl EnvironmentSpec {
    /// Resolves the EnvironmentSpec to a relative path based on the
    /// provided source anchor.
    pub fn resolve(mut self, source_anchor: SourceAnchor) -> Self {
        let maybe_source_spec = self.requirement.1.try_into_source_spec();
        let pixi_spec = match maybe_source_spec {
            Ok(source_spec) => {
                let resolved_spec = source_anchor.resolve(source_spec);
                PixiSpec::from(resolved_spec)
            }
            Err(pixi_spec) => pixi_spec,
        };
        self.requirement.1 = pixi_spec;
        self
    }
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
            command: CommandSpec::EnvironmentSpec(Box::new(EnvironmentSpec {
                requirement: (DEFAULT_BUILD_TOOL.parse().unwrap(), PixiSpec::any()),
                additional_requirements: Default::default(),
                constraints: Default::default(),
                channels: vec![conda_forge_channel, backends_channel],
                command: None,
            })),
        }
    }
}
