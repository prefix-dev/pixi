use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use miette::Diagnostic;
use pixi_build_type_conversions::to_project_model_v1;
use pixi_build_types::ProjectModelV1;
use pixi_manifest::{
    DiscoveryStart, ExplicitManifestError, PrioritizedChannel, WithProvenance, WorkspaceDiscoverer,
    WorkspaceDiscoveryError,
};
use pixi_spec::SpecConversionError;
use rattler_conda_types::{ChannelConfig, ParseChannelError};
use thiserror::Error;
use pixi_spec_containers::DependencyMap;
use crate::{
    BackendSpec,
    backend_spec::{CommandSpec, EnvironmentSpec, JsonRpcBackendSpec},
};

const VALID_RECIPE_NAMES: [&str; 2] = ["recipe.yaml", "recipe.yml"];
const VALID_RECIPE_DIRS: [&str; 2] = ["", "recipe"];

/// Describes a backend discovered for a given source location.
#[derive(Debug)]
pub struct DiscoveredBackend {
    /// The specification of the backend. This is used to instantiate the build
    /// backend.
    pub backend_spec: BackendSpec,

    /// The parameters used to initialize the backend.
    pub init_params: BackendInitializationParams,
}

/// The parameters used to initialize a build backend
#[derive(Debug)]
pub struct BackendInitializationParams {
    /// The directory that contains the source code.
    pub source_dir: PathBuf,

    /// The path of the discovered manifest relative to the `source_dir`.
    pub manifest_path: PathBuf,

    /// Optionally, the manifest of the discovered package.
    pub project_model: Option<ProjectModelV1>,

    /// Additional configuration that applies to the backend.
    pub configuration: Option<serde_json::Value>,
}

/// Configuration to enable or disable certain protocols discovery.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct EnabledProtocols {
    /// Enable the rattler-build protocol.
    pub enable_rattler_build: bool,
    /// Enable the pixi protocol.
    pub enable_pixi: bool,
}

impl Default for EnabledProtocols {
    /// Create a new `EnabledProtocols` with all protocols enabled.
    fn default() -> Self {
        Self {
            enable_rattler_build: true,
            enable_pixi: true,
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum DiscoveryError {
    #[error("failed to discover a valid project manifest, the source path '{}' could not be found", .0.display()
    )]
    NotFound(PathBuf),

    #[error("depending on a `{0}` file but the rattler-build protocol is not enabled")]
    UnsupportedRecipeYaml(String),

    #[error(transparent)]
    #[diagnostic(transparent)]
    FailedToDiscoverPackage(#[from] WorkspaceDiscoveryError),

    #[error("the {} does not describe a package", .0.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("manifest")
    )]
    #[diagnostic(help("A [package] section is missing in the manifest"))]
    NotAPackage(PathBuf),

    #[error("encountered an invalid package manifest, {0}")]
    #[diagnostic(help("This is often caused by an internal error. Please report this issue."))]
    SpecConversionError(pixi_spec::SpecConversionError),

    #[error("the channel '{0}' could not be resolved, {1}")]
    InvalidChannel(String, ParseChannelError),

    #[error("the source directory does not contain a supported manifest")]
    #[diagnostic(help(
        "Ensure that the source directory contains a valid pixi.toml or meta.yaml file."
    ))]
    FailedToDiscover,
}

impl DiscoveredBackend {
    /// Try to discover a backend for the given source path.
    pub fn discover(
        source_path: &Path,
        channel_config: &ChannelConfig,
        enabled_protocols: &EnabledProtocols,
    ) -> Result<Self, DiscoveryError> {
        if !source_path.exists() {
            return Err(DiscoveryError::NotFound(source_path.to_path_buf()));
        }

        // If the user explicitly asked for a recipe.yaml file
        let source_file_name = source_path.file_name().and_then(OsStr::to_str);
        if let Some(source_file_name) = source_file_name {
            if VALID_RECIPE_NAMES.contains(&source_file_name) {
                if !enabled_protocols.enable_rattler_build {
                    return Err(DiscoveryError::UnsupportedRecipeYaml(
                        source_file_name.to_string(),
                    ));
                }
                let source_dir = source_path
                    .parent()
                    .expect("the recipe must live somewhere");
                return Self::from_recipe(source_dir, Path::new(source_file_name), channel_config);
            }
        }

        // Try to discover a pixi project.
        if enabled_protocols.enable_pixi {
            if let Some(pixi) = Self::discover_pixi(source_path, channel_config)? {
                return Ok(pixi);
            }
        }

        // Try to discover as a rattler-build recipe.
        if enabled_protocols.enable_rattler_build {
            if let Some(pixi) = Self::discover_rattler_build(source_path, channel_config)? {
                return Ok(pixi);
            }
        }

        Err(DiscoveryError::FailedToDiscover)
    }

    /// Construct a new instance based on a specific `recipe.yaml` file in the
    /// source directory.
    fn from_recipe(
        source_dir: &Path,
        recipe_relative_path: &Path,
        channel_config: &ChannelConfig,
    ) -> Result<Self, DiscoveryError> {
        Ok(Self {
            backend_spec: BackendSpec::JsonRpc(JsonRpcBackendSpec::default_rattler_build(
                channel_config,
            )),
            init_params: BackendInitializationParams {
                source_dir: source_dir.to_path_buf(),
                manifest_path: recipe_relative_path.to_path_buf(),
                project_model: None,
                configuration: None,
            },
        })
    }

    /// Try to discover a pixi.yoml file in the source directory.
    fn discover_pixi(
        source_path: &Path,
        channel_config: &ChannelConfig,
    ) -> Result<Option<Self>, DiscoveryError> {
        let manifests = match WorkspaceDiscoverer::new(DiscoveryStart::ExplicitManifest(
            source_path.to_path_buf(),
        ))
        .with_closest_package(true)
        .discover()
        {
            Ok(None)
            | Err(WorkspaceDiscoveryError::ExplicitManifestError(
                ExplicitManifestError::InvalidManifest(_),
            )) => return Ok(None),
            Err(e) => return Err(DiscoveryError::FailedToDiscoverPackage(e)),
            Ok(Some(workspace)) => workspace.value,
        };

        // Make sure the manifest describes a package.
        let Some(WithProvenance {
            value: package_manifest,
            provenance,
        }) = manifests.package
        else {
            return Err(DiscoveryError::NotAPackage(
                manifests.workspace.provenance.path,
            ));
        };

        // Construct the project model from the manifest
        let project_model = to_project_model_v1(&package_manifest, channel_config)
            .map_err(DiscoveryError::SpecConversionError)?;

        // If we get here the tool is not overridden, so we use the isolated variant
        let build_system = package_manifest.build;
        let requirement = (
            build_system.backend.name.clone(),
            build_system
                .backend
                .spec
                .try_into_nameless_match_spec(channel_config)
                .map_err(DiscoveryError::SpecConversionError)?,
        );
        let additional_requirements = build_system
            .additional_dependencies
            .into_iter()
            .map(|(name, spec)| Ok((name, spec.try_into_nameless_match_spec(channel_config)?)))
            .collect::<Result<_, SpecConversionError>>()
            .map_err(DiscoveryError::SpecConversionError)?;

        // Figure out the channels to use
        let named_channels = match build_system.channels.as_ref() {
            Some(channels) => itertools::Either::Left(channels.iter()),
            None => itertools::Either::Right(PrioritizedChannel::sort_channels_by_priority(
                manifests.workspace.value.workspace.channels.iter(),
            )),
        };
        let channels = named_channels
            .map(|channel| {
                channel
                    .clone()
                    .into_base_url(channel_config)
                    .map_err(|err| DiscoveryError::InvalidChannel(channel.to_string(), err))
            })
            .collect::<Result<_, _>>()?;

        // Make sure that the source directory is a directory.
        let source_dir = if source_path.is_file() {
            source_path
                .parent()
                .expect("a file has a parent")
                .to_path_buf()
        } else {
            source_path.to_path_buf()
        };

        Ok(Some(Self {
            backend_spec: BackendSpec::JsonRpc(JsonRpcBackendSpec {
                name: build_system.backend.name.as_normalized().to_string(),
                command: CommandSpec::EnvironmentSpec(EnvironmentSpec {
                    requirement,
                    additional_requirements,
                    channels,
                    constraints: DependencyMap::default(),
                    command: None,
                }),
            }),
            init_params: BackendInitializationParams {
                manifest_path: pathdiff::diff_paths(provenance.path, &source_dir).expect(
                    "must be able to construct a path to go from source dir to manifest path",
                ),
                source_dir,
                project_model: Some(project_model),
                configuration: build_system.configuration.map(|config| {
                    config
                        .deserialize_into()
                        .expect("Configuration dictionary should be serializable to JSON")
                }),
            },
        }))
    }

    /// Try to discover a rattler build recipe in the repository.
    fn discover_rattler_build(
        source_dir: &Path,
        channel_config: &ChannelConfig,
    ) -> Result<Option<Self>, DiscoveryError> {
        for (&recipe_dir, &recipe_file) in VALID_RECIPE_DIRS.iter().zip(VALID_RECIPE_NAMES.iter()) {
            let recipe_path = source_dir.join(recipe_dir).join(recipe_file);
            if recipe_path.is_file() {
                return Ok(Some(Self::from_recipe(
                    source_dir,
                    &recipe_path,
                    channel_config,
                )?));
            }
        }
        Ok(None)
    }
}
