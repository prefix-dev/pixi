use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use itertools::Itertools;
use miette::Diagnostic;
use ordermap::OrderMap;
use pixi_build_type_conversions::{to_project_model_v1, to_target_selector_v1};
use pixi_build_types::{ProjectModelV1, TargetSelectorV1};
use pixi_manifest::{
    DiscoveryStart, ExplicitManifestError, PackageManifest, PrioritizedChannel, WithProvenance,
    WorkspaceDiscoverer, WorkspaceDiscoveryError, WorkspaceManifest,
};
use pixi_spec::{SourceLocationSpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::ChannelConfig;
use thiserror::Error;

use crate::{
    BackendSpec,
    backend_spec::{CommandSpec, EnvironmentSpec, JsonRpcBackendSpec},
};

const VALID_RECIPE_NAMES: [&str; 2] = ["recipe.yaml", "recipe.yml"];
const VALID_RECIPE_DIRS: [&str; 2] = ["", "recipe"];

/// Describes a backend discovered for a given source location.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub struct DiscoveredBackend {
    /// The specification of the backend. This is used to instantiate the build
    /// backend.
    pub backend_spec: BackendSpec,

    /// The parameters used to initialize the backend.
    pub init_params: BackendInitializationParams,
}

/// The parameters used to initialize a build backend
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub struct BackendInitializationParams {
    /// The root directory of the workspace.
    pub workspace_root: PathBuf,

    /// The location of the source code.
    pub source: Option<SourceLocationSpec>,

    /// The anchor for relative paths to the location of the source code.
    pub source_anchor: PathBuf,

    /// The absolute path of the discovered manifest
    pub manifest_path: PathBuf,

    /// Optionally, the manifest of the discovered package.
    pub project_model: Option<ProjectModelV1>,

    /// Additional configuration that applies to the backend.
    pub configuration: Option<serde_json::Value>,

    /// Targets that apply to the backend.
    pub target_configuration: Option<OrderMap<TargetSelectorV1, serde_json::Value>>,
}

/// Configuration to enable or disable certain protocols discovery.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
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

    #[error("the source directory '{0}', does not contain a supported manifest")]
    #[diagnostic(help(
        "Ensure that the source directory contains a valid pixi.toml or recipe.yaml file."
    ))]
    FailedToDiscover(String),
}

impl DiscoveredBackend {
    /// Try to discover a backend for the given source path.
    pub fn discover(
        source_path: &Path,
        channel_config: &ChannelConfig,
        enabled_protocols: &EnabledProtocols,
    ) -> Result<Self, DiscoveryError> {
        let Ok(source_path) = dunce::canonicalize(source_path) else {
            return Err(DiscoveryError::NotFound(source_path.to_path_buf()));
        };

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
                return Self::from_recipe(source_dir.to_path_buf(), source_path, channel_config);
            }
        }

        // Try to discover a pixi project.
        if enabled_protocols.enable_pixi {
            if let Some(pixi) = Self::discover_pixi(source_path.clone(), channel_config)? {
                return Ok(pixi);
            }
        }

        // Try to discover as a rattler-build recipe.
        if enabled_protocols.enable_rattler_build {
            if let Some(pixi) = Self::discover_rattler_build(source_path.clone(), channel_config)? {
                return Ok(pixi);
            }
        }

        Err(DiscoveryError::FailedToDiscover(
            source_path.to_string_lossy().to_string(),
        ))
    }

    /// Construct a new instance based on a specific `recipe.yaml` file in the
    /// source directory.
    fn from_recipe(
        source_dir: PathBuf,
        recipe_absolute_path: PathBuf,
        channel_config: &ChannelConfig,
    ) -> Result<Self, DiscoveryError> {
        debug_assert!(source_dir.is_absolute());
        debug_assert!(recipe_absolute_path.is_absolute());
        Ok(Self {
            backend_spec: BackendSpec::JsonRpc(JsonRpcBackendSpec::default_rattler_build(
                channel_config,
            )),
            init_params: BackendInitializationParams {
                workspace_root: source_dir.clone(),
                source: None,
                source_anchor: source_dir,
                manifest_path: recipe_absolute_path,
                project_model: None,
                configuration: None,
                target_configuration: None,
            },
        })
    }

    /// Convert a package manifest and corresponding workspace manifest into a
    /// discovered backend, with optional platform-specific configuration.
    pub fn from_package_and_workspace(
        // source_path: PathBuf,
        package_manifest: &WithProvenance<PackageManifest>,
        workspace: &WithProvenance<WorkspaceManifest>,
        channel_config: &ChannelConfig,
    ) -> Result<Self, SpecConversionError> {
        let WithProvenance {
            value: package_manifest,
            provenance,
        } = package_manifest;

        let workspace_root = workspace
            .provenance
            .path
            .parent()
            .expect("workspace manifest should have a parent directory")
            .to_path_buf();

        // Construct the project model from the manifest
        let project_model = to_project_model_v1(package_manifest, channel_config)?;

        // Determine the build system requirements.
        let build_system = package_manifest.build.clone();
        let requirement = (
            build_system.backend.name.clone(),
            build_system.backend.spec.clone(),
        );
        let additional_requirements = build_system.additional_dependencies.into_iter().collect();

        // Figure out the channels to use
        let named_channels = match build_system.channels.as_ref() {
            Some(channels) => itertools::Either::Left(channels.iter()),
            None => itertools::Either::Right(PrioritizedChannel::sort_channels_by_priority(
                workspace.value.workspace.channels.iter(),
            )),
        };
        let channels = named_channels
            .map(|channel| {
                channel
                    .clone()
                    .into_base_url(channel_config)
                    .map_err(|err| SpecConversionError::InvalidChannel(channel.to_string(), err))
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            backend_spec: BackendSpec::JsonRpc(JsonRpcBackendSpec {
                name: build_system.backend.name.as_normalized().to_string(),
                command: CommandSpec::EnvironmentSpec(Box::new(EnvironmentSpec {
                    requirement,
                    additional_requirements,
                    channels,
                    constraints: DependencyMap::default(),
                    command: None,
                })),
            }),
            init_params: BackendInitializationParams {
                workspace_root,
                manifest_path: provenance.path.clone(),
                source: build_system.source,
                source_anchor: provenance
                    .path
                    .parent()
                    .expect("points to a file")
                    .to_path_buf(),
                project_model: Some(project_model),
                configuration: build_system.config.map(|config| {
                    config
                        .deserialize_into()
                        .expect("Configuration dictionary needs to be serializable to JSON")
                }),
                target_configuration: build_system.target_config.map(|c| {
                    c.into_iter()
                        .map(|(selector, config)| {
                            (
                                to_target_selector_v1(&selector),
                                config.deserialize_into().expect(
                                    "Configuration dictionary needs to be serializable to JSON",
                                ),
                            )
                        })
                        .collect()
                }),
            },
        })
    }

    /// Try to discover a pixi.toml file with a `[package]` table in the source
    /// directory.
    fn discover_pixi(
        source_path: PathBuf,
        channel_config: &ChannelConfig,
    ) -> Result<Option<Self>, DiscoveryError> {
        let manifests =
            match WorkspaceDiscoverer::new(DiscoveryStart::ExplicitManifest(source_path.clone()))
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
        let Some(package_manifest) = manifests.package else {
            return Err(DiscoveryError::NotAPackage(
                manifests.workspace.provenance.path,
            ));
        };

        Self::from_package_and_workspace(
            // source_path,
            &package_manifest,
            &manifests.workspace,
            channel_config,
        )
        .map_err(DiscoveryError::SpecConversionError)
        .map(Some)
    }

    /// Try to discover a rattler build recipe in the repository.
    fn discover_rattler_build(
        source_dir: PathBuf,
        channel_config: &ChannelConfig,
    ) -> Result<Option<Self>, DiscoveryError> {
        for (&recipe_dir, &recipe_file) in VALID_RECIPE_DIRS
            .iter()
            .cartesian_product(VALID_RECIPE_NAMES.iter())
        {
            let recipe_path = source_dir.join(recipe_dir).join(recipe_file);
            if recipe_path.is_file() {
                return Ok(Some(Self::from_recipe(
                    source_dir,
                    recipe_path,
                    channel_config,
                )?));
            }
        }
        Ok(None)
    }
}
