use std::{
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::Diagnostic;
use pixi_manifest::{
    DiscoveryStart, ExplicitManifestError, PackageManifest, PrioritizedChannel,
    WorkspaceDiscoverer, WorkspaceDiscoveryError, WorkspaceManifest,
};
use rattler_conda_types::{ChannelConfig, MatchSpec};
use thiserror::Error;
use which::Error;

use crate::{
    backend_override::BackendOverride,
    jsonrpc::{Receiver, Sender},
    protocols::{InitializeError, JsonRPCBuildProtocol},
    tool::{IsolatedToolSpec, ToolCacheError, ToolSpec},
    InProcessBackend, ToolContext,
};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub struct ProtocolBuilder {
    source_dir: PathBuf,
    manifest_path: PathBuf,
    workspace_manifest: WorkspaceManifest,
    package_manifest: PackageManifest,
    configuration: Option<serde_json::Value>,
    backend_override: Option<BackendOverride>,
    channel_config: Option<ChannelConfig>,
    cache_dir: Option<PathBuf>,
}

#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum ProtocolBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    FailedToDiscoverPackage(#[from] WorkspaceDiscoveryError),

    #[error("the {} does not describe a package", .0.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("manifest")
    )]
    #[diagnostic(help("A [package] section is missing in the manifest"))]
    NotAPackage(PathBuf),
}

#[derive(Debug, Error, Diagnostic)]
pub enum FinishError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Init(#[from] InitializeError),
    NoBuildSection(PathBuf),
    Tool(ToolCacheError),
    SpecConversionError(pixi_spec::SpecConversionError),
}

impl Display for FinishError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FinishError::Init(init) => write!(f, "{init}"),
            FinishError::NoBuildSection(_) => write!(f, "failed to setup a build backend, the project manifest does not contain a [build-system] section"),
            FinishError::Tool(ToolCacheError::Instantiate(tool, err)) => match err {
                Error::CannotGetCurrentDirAndPathListEmpty | Error::CannotFindBinaryPath => write!(f, "failed to setup a build backend, the backend tool '{}' could not be found", tool.display()),
                Error::CannotCanonicalize => write!(f, "failed to setup a build backend, although the backend tool  '{}' can be resolved it could not be canonicalized", tool.display()),
            },
            FinishError::Tool(ToolCacheError::Install(report)) => write!(f, "failed to setup a build backend, the backend tool could not be installed: {}", report),
            FinishError::Tool(ToolCacheError::CacheDir(report)) => write!(f, "failed to setup a build backend, the cache dir could not be discovered: {}", report),
            FinishError::SpecConversionError(err) => write!(f, "failed to setup a build backend, the backend tool spec could not be converted: {}", err),
        }
    }
}

impl ProtocolBuilder {
    /// Constructs a new instance from a manifest.
    pub fn new(
        source_dir: PathBuf,
        manifest_path: PathBuf,
        workspace_manifest: WorkspaceManifest,
        package_manifest: PackageManifest,
    ) -> Self {
        let configuration = package_manifest.build.configuration.clone().map(|v| {
            v.deserialize_into()
                .expect("Configuration dictionary should be serializable to JSON")
        });

        Self {
            source_dir,
            manifest_path,
            workspace_manifest,
            package_manifest,
            configuration,
            backend_override: None,
            channel_config: None,
            cache_dir: None,
        }
    }

    /// Sets the configuration of the build backend
    pub fn with_configuration(self, config: serde_json::Value) -> Self {
        Self {
            configuration: Some(config),
            ..self
        }
    }

    /// Sets an optional backend override.
    pub fn with_backend_override(self, backend_override: BackendOverride) -> Self {
        Self {
            backend_override: Some(backend_override),
            ..self
        }
    }

    /// Sets the channel configuration used by this instance.
    pub fn with_channel_config(self, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config: Some(channel_config),
            ..self
        }
    }

    /// Sets the cache directory the backend should use.
    pub fn with_opt_cache_dir(self, cache_dir: Option<PathBuf>) -> Self {
        Self { cache_dir, ..self }
    }

    /// Discovers a pixi project in the given source directory.
    pub fn discover(source_dir: &Path) -> Result<Option<Self>, ProtocolBuildError> {
        let manifests = match WorkspaceDiscoverer::new(DiscoveryStart::ExplicitManifest(
            source_dir.to_path_buf(),
        ))
        .with_closest_package(true)
        .discover()
        {
            Ok(None)
            | Err(WorkspaceDiscoveryError::ExplicitManifestError(
                ExplicitManifestError::InvalidManifest(_),
            )) => return Ok(None),
            Err(e) => return Err(ProtocolBuildError::FailedToDiscoverPackage(e)),
            Ok(Some(workspace)) => workspace.value,
        };

        // Make sure the manifest describes a package.
        let Some(package_manifest) = manifests.package else {
            return Err(ProtocolBuildError::NotAPackage(
                manifests.workspace.provenance.path,
            ));
        };

        let builder = Self::new(
            source_dir.to_path_buf(),
            package_manifest.provenance.path,
            manifests.workspace.value,
            package_manifest.value,
        );

        Ok(Some(builder))
    }

    fn get_tool_spec(&self, channel_config: &ChannelConfig) -> Result<ToolSpec, FinishError> {
        // The tool is either overridden or its not, with pixi the backend is specified
        // in the toml so it's unclear if we need to override the tool until
        // this point, lets check that now
        if let Some(backend_override) = self.backend_override.as_ref() {
            let tool_name = self.package_manifest.build.backend.name.clone();
            if let Some(tool) = backend_override.overridden_tool(tool_name.as_normalized()) {
                return Ok(tool.as_spec());
            }
        }

        // If we get here the tool is not overridden, so we use the isolated variant
        let build_system = &self.package_manifest.build;
        let specs = [(&build_system.backend.name, &build_system.backend.spec)]
            .into_iter()
            .chain(build_system.additional_dependencies.iter())
            .map(|(name, spec)| {
                spec.clone()
                    .try_into_nameless_match_spec(channel_config)
                    .map(|spec| MatchSpec::from_nameless(spec, Some(name.clone())))
            })
            .collect::<Result<_, _>>()
            .map_err(FinishError::SpecConversionError)?;

        // Figure out the channels to use
        let channels = build_system.channels.clone().unwrap_or_else(|| {
            PrioritizedChannel::sort_channels_by_priority(
                self.workspace_manifest.workspace.channels.iter(),
            )
            .cloned()
            .collect()
        });

        Ok(ToolSpec::Isolated(IsolatedToolSpec {
            specs,
            command: build_system.backend.name.as_source().to_string(),
            channels,
        }))
    }

    pub async fn finish(
        self,
        tool: Arc<ToolContext>,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, FinishError> {
        let channel_config = self.channel_config.clone().unwrap_or_else(|| {
            ChannelConfig::default_with_root_dir(
                self.manifest_path
                    .parent()
                    .expect("a manifest must always reside in a directory")
                    .to_path_buf(),
            )
        });

        let tool_spec = self.get_tool_spec(&channel_config)?;

        let tool = tool
            .instantiate(tool_spec, &channel_config)
            .await
            .map_err(FinishError::Tool)?;

        let channel_config = self
            .channel_config
            .clone()
            .unwrap_or_else(|| ChannelConfig::default_with_root_dir(self.source_dir.clone()));

        Ok(JsonRPCBuildProtocol::setup(
            self.source_dir,
            self.manifest_path,
            Some(&self.package_manifest),
            self.configuration,
            &channel_config,
            build_id,
            self.cache_dir,
            tool,
        )
        .await?)
    }

    /// Finish the construction of the protocol with the given
    /// [`InProcessBackend`] representing the build backend.
    pub async fn finish_with_ipc(
        self,
        ipc: InProcessBackend,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, FinishError> {
        let channel_config = self
            .channel_config
            .clone()
            .unwrap_or_else(|| ChannelConfig::default_with_root_dir(self.source_dir.clone()));

        Ok(JsonRPCBuildProtocol::setup_with_transport(
            "<IPC>".to_string(),
            self.source_dir,
            self.manifest_path,
            Some(&self.package_manifest),
            self.configuration,
            &channel_config,
            build_id,
            self.cache_dir,
            Sender::from(ipc.rpc_out),
            Receiver::from(ipc.rpc_in),
            None,
        )
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ProtocolBuilder;

    #[test]
    pub fn discover_basic_pixi_manifest() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/basic");
        ProtocolBuilder::discover(&manifest_dir).unwrap();
    }
}
