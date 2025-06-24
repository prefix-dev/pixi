use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use miette::Diagnostic;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::{
    Backend,
    types::{
        ChannelConfiguration, PlatformAndVirtualPackages,
        procedures::conda_metadata::CondaMetadataParams,
    },
};
use pixi_build_types::procedures::conda_outputs::CondaOutputsParams;
use pixi_glob::GlobHashKey;
use pixi_record::{InputHash, PinnedSourceSpec};
use rand::random;
use rattler_conda_types::{ChannelConfig, ChannelUrl};
use thiserror::Error;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    InstantiateBackendError, InstantiateBackendSpec, SourceCheckout, SourceCheckoutError,
    build::{
        WorkDirKey,
        source_metadata_cache::{self, CachedCondaMetadata, MetadataKind, SourceMetadataKey},
    },
    lazy_source_checkout::LazySourceCheckout,
};

/// Represents a request for metadata from a build backend for a particular
/// source location. The result of this request is the metadata for that
/// particular source.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct BuildBackendMetadataSpec {
    /// The source specification
    pub source: PinnedSourceSpec,

    /// The channel configuration to use for the build backend.
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about the build environment.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The protocols that are enabled for this source
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

/// The metadata of a source checkout.
#[derive(Debug)]
pub struct BuildBackendMetadata {
    /// The source checkout that the manifest was extracted from.
    pub source: PinnedSourceSpec,

    /// The cache entry that contains the metadata acquired from the build
    /// backend.
    ///
    /// As long as the cache entry is not dropped, the metadata cannot be
    /// accessed by another process.
    pub cache_entry: source_metadata_cache::CacheEntry,

    /// The metadata that was acquired from the build backend.
    pub metadata: CachedCondaMetadata,
}

impl BuildBackendMetadataSpec {
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<BuildBackendMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        tracing::debug!(
            "Requesting source metadata for source spec: {}",
            self.source
        );

        // Create a lazy source checkout that can be used to check out the source only
        // when needed. Not all code paths require a full source checkout.
        let mut lazy_source = LazySourceCheckout::new(self.source.clone());

        // Check the source metadata cache, short circuit if there is a cache hit that
        // is still fresh.
        let cache_key = self.cache_key();
        let (metadata, mut cache_entry) = command_dispatcher
            .source_metadata_cache()
            .entry(&cache_key)
            .await
            .map_err(BuildBackendMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;
        if let Some(metadata) =
            Self::verify_cache_freshness(&mut lazy_source, &command_dispatcher, metadata).await?
        {
            return Ok(BuildBackendMetadata {
                metadata,
                cache_entry,
                source: self.source,
            });
        }

        // Ensure that the source is checked out before proceeding.
        let source_checkout = lazy_source
            .into_checkout(&command_dispatcher)
            .await
            .map_err_with(BuildBackendMetadataError::SourceCheckout)?;

        // Discover information about the build backend from the source code.
        let discovered_backend = DiscoveredBackend::discover(
            &source_checkout.path,
            &self.channel_config,
            &self.enabled_protocols,
        )
        .map_err(BuildBackendMetadataError::Discovery)
        .map_err(CommandDispatcherError::Failed)?;

        // Instantiate the backend with the discovered information.
        let manifest_path = discovered_backend.init_params.manifest_path.clone();
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend.backend_spec,
                init_params: discovered_backend.init_params,
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
            .map_err_with(BuildBackendMetadataError::Initialize)?;

        // Based on the version of the backend, call the appropriate method to get
        // metadata.
        let source = self.source.clone();
        let metadata = if backend.api_version.supports_conda_outputs() {
            self.call_conda_outputs(command_dispatcher, source_checkout, manifest_path, backend)
                .await?
        } else {
            self.call_conda_get_metadata(
                command_dispatcher,
                source_checkout,
                manifest_path,
                backend,
            )
            .await?
        };

        // Store the metadata in the cache for later retrieval
        cache_entry
            .write(metadata.clone())
            .await
            .map_err(BuildBackendMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(BuildBackendMetadata {
            metadata,
            cache_entry,
            source,
        })
    }

    async fn verify_cache_freshness(
        source: &mut LazySourceCheckout,
        command_dispatcher: &CommandDispatcher,
        metadata: Option<CachedCondaMetadata>,
    ) -> Result<Option<CachedCondaMetadata>, CommandDispatcherError<BuildBackendMetadataError>>
    {
        let Some(metadata) = metadata else {
            return Ok(None);
        };

        tracing::debug!(
            "Found source metadata in cache for source spec: {}",
            source.as_pinned()
        );

        let Some(input_globs) = &metadata.input_hash else {
            // No input hash so just assume it is still valid.
            tracing::debug!("found cached metadata.");
            return Ok(Some(metadata));
        };

        // Get the source code to check the globs.
        let source_checkout = source
            .checkout(command_dispatcher)
            .await
            .map_err_with(BuildBackendMetadataError::SourceCheckout)?;

        // Check if the input hash is still valid.
        let new_hash = command_dispatcher
            .glob_hash_cache()
            .compute_hash(GlobHashKey::new(
                source_checkout.path.clone(),
                input_globs.globs.clone(),
            ))
            .await
            .map_err(BuildBackendMetadataError::GlobHash)
            .map_err(CommandDispatcherError::Failed)?;

        if new_hash.hash == input_globs.hash {
            tracing::debug!("found up-to-date cached metadata.");
            Ok(Some(metadata))
        } else {
            tracing::debug!("found stale cached metadata.");
            Ok(None)
        }
    }

    /// Use the `conda/outputs` procedure to get the metadata for the source
    /// checkout.
    async fn call_conda_outputs(
        self,
        command_dispatcher: CommandDispatcher,
        source_checkout: SourceCheckout,
        manifest_path: PathBuf,
        backend: Backend,
    ) -> Result<CachedCondaMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        let params = CondaOutputsParams {
            host_platform: self.build_environment.host_platform,
            variant_configuration: self.variants.map(|variants| variants.into_iter().collect()),
            work_directory: command_dispatcher.cache_dirs().working_dirs().join(
                WorkDirKey {
                    source: Box::new(source_checkout.clone()).into(),
                    host_platform: self.build_environment.host_platform,
                    build_backend: backend.identifier().to_string(),
                }
                .key(),
            ),
        };
        let outputs = backend
            .conda_outputs(params)
            .await
            .map_err(BuildBackendMetadataError::Communication)
            .map_err(CommandDispatcherError::Failed)?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = Self::compute_input_hash(
            command_dispatcher,
            &source_checkout,
            manifest_path,
            outputs.input_globs.clone(),
        )
        .await?;

        Ok(CachedCondaMetadata {
            id: random(),
            input_hash: input_hash.clone(),
            metadata: MetadataKind::Outputs {
                outputs: outputs.outputs,
            },
        })
    }

    /// Use the `conda/getMetadata` procedure to get the metadata for the source
    async fn call_conda_get_metadata(
        self,
        command_dispatcher: CommandDispatcher,
        source_checkout: SourceCheckout,
        manifest_path: PathBuf,
        backend: Backend,
    ) -> Result<CachedCondaMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        // Query the backend for metadata.
        let params = CondaMetadataParams {
            build_platform: Some(PlatformAndVirtualPackages {
                platform: self.build_environment.build_platform,
                virtual_packages: Some(self.build_environment.build_virtual_packages),
            }),
            host_platform: Some(PlatformAndVirtualPackages {
                platform: self.build_environment.host_platform,
                virtual_packages: Some(self.build_environment.host_virtual_packages),
            }),
            channel_base_urls: Some(self.channels.into_iter().map(Into::into).collect()),
            channel_configuration: ChannelConfiguration {
                base_url: self.channel_config.channel_alias.clone(),
            },
            variant_configuration: self.variants.map(|variants| variants.into_iter().collect()),
            work_directory: command_dispatcher.cache_dirs().working_dirs().join(
                WorkDirKey {
                    source: Box::new(source_checkout.clone()).into(),
                    host_platform: self.build_environment.host_platform,
                    build_backend: backend.identifier().to_string(),
                }
                .key(),
            ),
        };
        let metadata = backend
            .conda_get_metadata(params)
            .await
            .map_err(BuildBackendMetadataError::Communication)
            .map_err(CommandDispatcherError::Failed)?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = Self::compute_input_hash(
            command_dispatcher,
            &source_checkout,
            manifest_path,
            metadata.input_globs.clone(),
        )
        .await?;

        Ok(CachedCondaMetadata {
            id: random(),
            input_hash: input_hash.clone(),
            metadata: MetadataKind::GetMetadata {
                packages: metadata.packages,
            },
        })
    }

    /// Computes the input hash for metadata returned by the backend.
    async fn compute_input_hash(
        command_queue: CommandDispatcher,
        source: &SourceCheckout,
        manifest_path: PathBuf,
        input_globs: Option<BTreeSet<String>>,
    ) -> Result<Option<InputHash>, CommandDispatcherError<BuildBackendMetadataError>> {
        let input_hash = if source.pinned.is_immutable() {
            None
        } else {
            // Compute the input hash based on the manifest path and the input globs.
            let mut input_globs = input_globs.unwrap_or_default();
            input_globs.insert(manifest_path.to_string_lossy().into_owned());
            let input_hash = command_queue
                .glob_hash_cache()
                .compute_hash(GlobHashKey::new(&source.path, input_globs.clone()))
                .await
                .map_err(BuildBackendMetadataError::GlobHash)
                .map_err(CommandDispatcherError::Failed)?;

            Some(InputHash {
                hash: input_hash.hash,
                globs: input_globs,
            })
        };
        Ok(input_hash)
    }

    /// Computes the cache key for this instance
    pub(crate) fn cache_key(&self) -> SourceMetadataKey {
        SourceMetadataKey {
            channel_urls: self.channels.clone(),
            build_environment: self.build_environment.clone(),
            build_variants: self.variants.clone().unwrap_or_default(),
            enabled_protocols: self.enabled_protocols.clone(),
            pinned_source: self.source.clone(),
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildBackendMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(#[from] pixi_build_discovery::DiscoveryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Initialize(#[from] InstantiateBackendError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Communication(#[from] pixi_build_frontend::json_rpc::CommunicationError),

    #[error("could not compute hash of input files")]
    GlobHash(#[from] pixi_glob::GlobHashError),

    #[error(transparent)]
    Cache(#[from] source_metadata_cache::SourceMetadataCacheError),
}
