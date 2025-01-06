use std::{
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::Diagnostic;
use pixi_consts::consts;
use pixi_manifest::{Manifest, PackageManifest, PrioritizedChannel, WorkspaceManifest};
use rattler_conda_types::{ChannelConfig, MatchSpec};
use thiserror::Error;
use which::Error;

use crate::{
    jsonrpc::{Receiver, Sender},
    protocols::{InitializeError, JsonRPCBuildProtocol},
    tool::{IsolatedToolSpec, ToolCacheError, ToolSpec},
    BackendOverride, InProcessBackend, ToolContext,
};
// use super::{InitializeError, JsonRPCBuildProtocol};

/// A protocol that uses a pixi manifest to invoke a build backend .
#[derive(Debug)]
pub struct ProtocolBuilder {
    source_dir: PathBuf,
    manifest_path: PathBuf,
    workspace_manifest: WorkspaceManifest,
    package_manifest: PackageManifest,
    override_backend_spec: Option<ToolSpec>,
    channel_config: Option<ChannelConfig>,
    cache_dir: Option<PathBuf>,
}

#[derive(thiserror::Error, Debug, Diagnostic)]
pub enum ProtocolBuildError {
    #[error("failed to setup a build backend, the {} could not be parsed", .0.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("manifest")
    )]
    #[diagnostic(help("Ensure that the manifest at '{}' is a valid pixi project manifest", .0.display()
    ))]
    FailedToParseManifest(PathBuf, #[diagnostic_source] miette::Report),

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
        Self {
            source_dir,
            manifest_path,
            workspace_manifest,
            package_manifest,
            override_backend_spec: None,
            channel_config: None,
            cache_dir: None,
        }
    }

    /// Sets an optional backend override.
    pub fn with_backend_override(self, backend_override: Option<BackendOverride>) -> Self {
        Self {
            override_backend_spec: backend_override
                .map(BackendOverride::into_spec)
                .or(self.override_backend_spec),
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
        if let Some(manifest_path) = find_pixi_manifest(source_dir) {
            match Manifest::from_path(&manifest_path) {
                Ok(manifest) => {
                    // Make sure the manifest describes a package.
                    let Some(package_manifest) = manifest.package else {
                        return Err(ProtocolBuildError::NotAPackage(manifest_path));
                    };

                    let builder = Self::new(
                        source_dir.to_path_buf(),
                        manifest_path,
                        manifest.workspace,
                        package_manifest,
                    );
                    return Ok(Some(builder));
                }
                Err(e) => {
                    return Err(ProtocolBuildError::FailedToParseManifest(
                        manifest_path.to_path_buf(),
                        e,
                    ));
                }
            }
        }
        Ok(None)
    }

    pub async fn finish(
        self,
        tool: Arc<ToolContext>,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, FinishError> {
        let channel_config = self.channel_config.unwrap_or_else(|| {
            ChannelConfig::default_with_root_dir(
                self.manifest_path
                    .parent()
                    .expect("a manifest must always reside in a directory")
                    .to_path_buf(),
            )
        });

        let tool_spec = if let Some(backend_spec) = self.override_backend_spec {
            backend_spec
        } else {
            let build_system = &self.package_manifest.build;
            let specs = [(&build_system.backend.name, &build_system.backend.spec)]
                .into_iter()
                .chain(build_system.additional_dependencies.iter())
                .map(|(name, spec)| {
                    spec.clone()
                        .try_into_nameless_match_spec(&channel_config)
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

            ToolSpec::Isolated(IsolatedToolSpec {
                specs,
                command: build_system.backend.name.as_source().to_string(),
                channels,
            })
        };

        let tool = tool
            .instantiate(tool_spec, &channel_config)
            .await
            .map_err(FinishError::Tool)?;

        Ok(JsonRPCBuildProtocol::setup(
            self.source_dir,
            self.manifest_path,
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
        Ok(JsonRPCBuildProtocol::setup_with_transport(
            "<IPC>".to_string(),
            self.source_dir,
            self.manifest_path,
            build_id,
            self.cache_dir,
            Sender::from(ipc.rpc_out),
            Receiver::from(ipc.rpc_in),
            None,
        )
        .await?)
    }
}

/// Try to find a pixi manifest in the given source directory.
fn find_pixi_manifest(source_dir: &Path) -> Option<PathBuf> {
    let pixi_manifest_path = source_dir.join(consts::PROJECT_MANIFEST);
    if pixi_manifest_path.exists() {
        return Some(pixi_manifest_path);
    }

    let pyproject_manifest_path = source_dir.join(consts::PYPROJECT_MANIFEST);
    // TODO: Really check if this is a pixi project.
    if pyproject_manifest_path.is_file() {
        return Some(pyproject_manifest_path);
    }

    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ProtocolBuilder;

    #[test]
    pub fn discover_basic_pixi_manifest() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/basic");
        let manifest_path = super::find_pixi_manifest(&manifest_dir)
            .unwrap_or_else(|| panic!("No manifest found at {}", manifest_dir.display()));
        ProtocolBuilder::discover(&manifest_path).unwrap();
    }
}
