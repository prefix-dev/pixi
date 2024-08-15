use std::path::PathBuf;

use jsonrpsee::{
    async_client::{Client, ClientBuilder},
    core::client::{ClientT, Error as ClientError, TransportReceiverT, TransportSenderT},
};
use miette::IntoDiagnostic;
use pixi_build_types::{
    procedures,
    procedures::{
        conda_metadata::{CondaMetadataParams, CondaMetadataResult},
        initialize::{InitializeParams, InitializeResult},
    },
    BackendCapabilities, FrontendCapabilities,
};
use pixi_manifest::Dependencies;
use pixi_spec::PixiSpec;
use rattler_conda_types::{ChannelConfig, MatchSpec, NoArchType, PackageName};

use crate::{
    jsonrpc::{stdio_transport, RpcParams},
    tool::Tool,
    CondaMetadata, CondaPackageMetadata,
};

#[derive(Debug, thiserror::Error)]
pub enum InitializeError {
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("JSON-RPC error")]
    JsonRpc(#[from] ClientError),
    #[error("Cannot acquire stdin handle")]
    StdinHandle,
    #[error("Cannot acquire stdout handle")]
    StdoutHandle,
}

/// A protocol that uses a pixi manifest to invoke a build backend.
/// and uses a JSON-RPC client to communicate with the backend.
pub struct Protocol {
    pub(super) channel_config: ChannelConfig,
    pub(super) client: Client,

    _backend_capabilities: BackendCapabilities,
}

impl Protocol {
    pub(crate) async fn setup(
        manifest_path: PathBuf,
        channel_config: ChannelConfig,
        tool: Tool,
    ) -> Result<Self, InitializeError> {
        // Spawn the tool and capture stdin/stdout.
        let process = tokio::process::Command::from(tool.command())
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit()) // TODO: Capture this?
            .spawn()?;

        // Acquire the stdin/stdout handles.
        let stdin = process.stdin.ok_or_else(|| InitializeError::StdinHandle)?;
        let stdout = process
            .stdout
            .ok_or_else(|| InitializeError::StdoutHandle)?;

        // Construct a JSON-RPC client to communicate with the backend process.
        let (tx, rx) = stdio_transport(stdin, stdout);

        Self::setup_with_transport(manifest_path, channel_config, tx, rx).await
    }

    pub async fn setup_with_transport(
        manifest_path: PathBuf,
        channel_config: ChannelConfig,
        sender: impl TransportSenderT + Send,
        receiver: impl TransportReceiverT + Send,
    ) -> Result<Self, InitializeError> {
        let client: Client = ClientBuilder::default()
            // Set 24hours for request timeout because the backend may be long-running.
            .request_timeout(std::time::Duration::from_secs(86400))
            .build_with_tokio(sender, receiver);

        // Invoke the initialize method on the backend to establish the connection.
        let result: InitializeResult = client
            .request(
                procedures::initialize::METHOD_NAME,
                RpcParams::from(InitializeParams {
                    manifest_path,
                    capabilities: FrontendCapabilities {},
                }),
            )
            .await?;

        Ok(Self {
            channel_config,
            client,
            _backend_capabilities: result.capabilities,
        })
    }

    /// Extract metadata from the recipe.
    pub async fn get_conda_metadata(
        &self,
        request: &CondaMetadataParams,
    ) -> miette::Result<CondaMetadata> {
        let result: CondaMetadataResult = self
            .client
            .request(
                procedures::conda_metadata::METHOD_NAME,
                RpcParams::from(request),
            )
            .await
            .into_diagnostic()?;

        Ok(CondaMetadata {
            packages: result
                .packages
                .into_iter()
                .map(|pkg| CondaPackageMetadata {
                    name: pkg.name,
                    version: pkg.version.into(),
                    build: pkg.build,
                    build_number: pkg.build_number,
                    subdir: pkg.subdir,
                    depends: pkg
                        .depends
                        .map(|specs| matchspecs_to_dependencies(specs, &self.channel_config))
                        .unwrap_or_default(),
                    constraints: pkg
                        .constrains
                        .map(|specs| matchspecs_to_dependencies(specs, &self.channel_config))
                        .unwrap_or_default(),
                    license: pkg.license,
                    license_family: pkg.license_family,
                    noarch: NoArchType::python(),
                })
                .collect(),
        })
    }
}

fn matchspecs_to_dependencies(
    matchspecs: Vec<MatchSpec>,
    channel_config: &ChannelConfig,
) -> Dependencies<PackageName, PixiSpec> {
    matchspecs
        .into_iter()
        .filter_map(|spec| {
            let (name, spec) = spec.into_nameless();
            Some((
                name?,
                PixiSpec::from_nameless_matchspec(spec, channel_config),
            ))
        })
        .collect()
}
