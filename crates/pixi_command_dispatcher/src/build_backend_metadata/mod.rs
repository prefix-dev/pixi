use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
};

use futures::{SinkExt, channel::mpsc::UnboundedSender};
use miette::Diagnostic;
use once_cell::sync::Lazy;
use pathdiff::diff_paths;
use pixi_build_discovery::{CommandSpec, EnabledProtocols};
use pixi_build_frontend::Backend;
use pixi_build_types::{ProjectModelV1, procedures::conda_outputs::CondaOutputsParams};
use pixi_glob::GlobHashKey;
use pixi_record::{InputHash, PinnedSourceSpec};
use pixi_spec::{SourceAnchor, SourceSpec};
use rand::random;
use rattler_conda_types::{ChannelConfig, ChannelUrl};
use thiserror::Error;
use tracing::instrument;
use xxhash_rust::xxh3::Xxh3;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    InstantiateBackendError, InstantiateBackendSpec, SourceCheckout, SourceCheckoutError,
    build::{
        SourceRecordOrCheckout, WorkDirKey,
        source_metadata_cache::{self, CachedCondaMetadata, MetadataKind, SourceMetadataKey},
    },
};
use pixi_build_discovery::BackendSpec;
use pixi_build_frontend::BackendOverride;

static WARNED_BACKENDS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn warn_once_per_backend(backend_name: &str) {
    let mut warned = WARNED_BACKENDS.lock().unwrap();
    if warned.insert(backend_name.to_string()) {
        tracing::warn!(
            "metadata cache disabled for build backend '{}' (system/path-based backends always regenerate metadata)",
            backend_name
        );
    }
}

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

    /// Variant file paths provided by the workspace.
    pub variant_files: Option<Vec<PathBuf>>,

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
    #[instrument(
        skip_all,
        name="backend-metadata",
        fields(
            source = %self.source,
            platform = %self.build_environment.host_platform,
        )
    )]
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
        log_sink: UnboundedSender<String>,
    ) -> Result<BuildBackendMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        // Ensure that the source is checked out before proceeding.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(self.source.clone())
            .await
            .map_err_with(BuildBackendMetadataError::SourceCheckout)?;

        // Discover information about the build backend from the source code (cached by path).
        let discovered_backend = command_dispatcher
            .discover_backend(
                &source_checkout.path,
                self.channel_config.clone(),
                self.enabled_protocols.clone(),
            )
            .await
            .map_err_with(BuildBackendMetadataError::Discovery)?;

        // Calculate the hash of the project model
        let additional_glob_hash = calculate_additional_glob_hash(
            &discovered_backend.init_params.project_model,
            &self.variants,
        );

        let glob_root = discovered_backend
            .init_params
            .glob_root_with_fallback(&source_checkout.path);

        // Check if we should skip the metadata cache for this backend
        let skip_cache = Self::should_skip_metadata_cache(
            &discovered_backend.backend_spec,
            command_dispatcher.build_backend_overrides(),
        );

        // Check the source metadata cache, short circuit if there is a cache hit that
        // is still fresh.
        let cache_key = self.cache_key();
        let (metadata, mut cache_entry) = command_dispatcher
            .source_metadata_cache()
            .entry(&cache_key)
            .await
            .map_err(BuildBackendMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        if !skip_cache {
            if let Some(metadata) =
                Self::verify_cache_freshness(&command_dispatcher, metadata, &additional_glob_hash)
                    .await?
            {
                return Ok(BuildBackendMetadata {
                    metadata,
                    cache_entry,
                    source: source_checkout.pinned,
                });
            }
        } else {
            let backend_name = match &discovered_backend.backend_spec {
                BackendSpec::JsonRpc(spec) => &spec.name,
            };
            warn_once_per_backend(backend_name);
        }

        // Instantiate the backend with the discovered information.
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend
                    .backend_spec
                    .clone()
                    .resolve(SourceAnchor::from(SourceSpec::from(self.source.clone()))),
                init_params: discovered_backend.init_params.clone(),
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
            })
            .await
            .map_err_with(BuildBackendMetadataError::Initialize)?;

        // Call the conda_outputs method to get metadata.
        let source = source_checkout.pinned.clone();
        if !backend.capabilities().provides_conda_outputs() {
            return Err(CommandDispatcherError::Failed(
                BuildBackendMetadataError::BackendMissingCapabilities(
                    backend.identifier().to_string(),
                ),
            ));
        }

        tracing::trace!(
            "Using `{}` procedure to get metadata information",
            pixi_build_types::procedures::conda_outputs::METHOD_NAME
        );
        let metadata = self
            .call_conda_outputs(
                command_dispatcher,
                source_checkout,
                backend,
                additional_glob_hash,
                glob_root,
                log_sink,
            )
            .await?;

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

    /// Checks if we should skip the metadata cache for this backend.
    /// Returns true if:
    /// 1. There's a System backend override (either for this specific backend or all backends)
    /// 2. OR the original backend spec is System or mutable (path-based non-binary)
    fn should_skip_metadata_cache(
        backend_spec: &BackendSpec,
        backend_override: &BackendOverride,
    ) -> bool {
        let BackendSpec::JsonRpc(json_rpc_spec) = backend_spec;

        // Check if there's a System backend override for this backend
        // In-memory overrides are deterministic and can use cached metadata
        let has_system_override = match backend_override {
            BackendOverride::System(overridden_backends) => overridden_backends
                .named_backend_override(&json_rpc_spec.name)
                .is_some(),
            BackendOverride::InMemory(_) => false,
        };

        let (command_kind, command_requires_skip) = match &json_rpc_spec.command {
            CommandSpec::System(_) => ("system", true),
            CommandSpec::EnvironmentSpec(env_spec) => {
                let mutable = env_spec.requirement.1.is_mutable();
                (
                    if mutable {
                        "mutable-environment"
                    } else {
                        "environment"
                    },
                    mutable,
                )
            }
        };

        let skip_cache = has_system_override || command_requires_skip;

        if skip_cache {
            let reason = if has_system_override {
                "override"
            } else {
                command_kind
            };
            tracing::debug!(
                backend = %json_rpc_spec.name,
                reason,
                command_kind,
                "metadata cache disabled for backend",
            );
        }

        skip_cache
    }

    async fn verify_cache_freshness(
        command_dispatcher: &CommandDispatcher,
        metadata: Option<CachedCondaMetadata>,
        additional_glob_hash: &[u8],
    ) -> Result<Option<CachedCondaMetadata>, CommandDispatcherError<BuildBackendMetadataError>>
    {
        let Some(metadata) = metadata else {
            return Ok(None);
        };

        let metadata_kind = match metadata.metadata {
            MetadataKind::GetMetadata { .. } => "conda/getMetadata",
            MetadataKind::Outputs { .. } => {
                pixi_build_types::procedures::conda_outputs::METHOD_NAME
            }
        };

        let Some(input_globs) = &metadata.input_hash else {
            // No input hash so just assume it is still valid.
            tracing::trace!("found cached `{metadata_kind}` response.");
            return Ok(Some(metadata));
        };

        let Some(cached_root) = metadata.glob_root.as_ref() else {
            tracing::debug!(
                "cached `{metadata_kind}` response missing glob root; regenerating metadata"
            );
            return Ok(None);
        };
        let effective_root = cached_root.as_path();

        // Check if the input hash is still valid.
        let new_hash = command_dispatcher
            .glob_hash_cache()
            .compute_hash(GlobHashKey::new(
                effective_root.to_path_buf(),
                input_globs.globs.clone(),
                additional_glob_hash.to_vec(),
            ))
            .await
            .map_err(BuildBackendMetadataError::GlobHash)
            .map_err(CommandDispatcherError::Failed)?;

        if new_hash.hash == input_globs.hash {
            tracing::trace!("found up-to-date cached `{metadata_kind}` response..");
            Ok(Some(metadata))
        } else {
            tracing::trace!("found stale `{metadata_kind}` response..");
            Ok(None)
        }
    }

    /// Use the `conda/outputs` procedure to get the metadata for the source
    /// checkout.
    async fn call_conda_outputs(
        self,
        command_dispatcher: CommandDispatcher,
        source_checkout: SourceCheckout,
        backend: Backend,
        additional_glob_hash: Vec<u8>,
        glob_root: PathBuf,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<CachedCondaMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        let backend_identifier = backend.identifier().to_string();
        let params = CondaOutputsParams {
            channels: self.channels,
            host_platform: self.build_environment.host_platform,
            build_platform: self.build_environment.build_platform,
            variant_configuration: self.variants.clone(),
            variant_files: self.variant_files.clone(),
            work_directory: command_dispatcher.cache_dirs().working_dirs().join(
                WorkDirKey {
                    source: SourceRecordOrCheckout::Checkout {
                        checkout: source_checkout.clone(),
                    },
                    host_platform: self.build_environment.host_platform,
                    build_backend: backend_identifier.clone(),
                }
                .key(),
            ),
        };
        let outputs = backend
            .conda_outputs(params, move |line| {
                let _err = futures::executor::block_on(log_sink.send(line));
            })
            .await
            .map_err(BuildBackendMetadataError::Communication)
            .map_err(CommandDispatcherError::Failed)?;

        for output in &outputs.outputs {
            tracing::debug!(
                backend = %backend_identifier,
                package = ?output.metadata.name,
                version = %output.metadata.version,
                build = %output.metadata.build,
                subdir = %output.metadata.subdir,
                "received metadata output from backend",
            );
        }

        // Compute the input globs for the mutable source checkouts.
        let input_globs = extend_input_globs_with_variant_files(
            outputs.input_globs.clone(),
            &self.variant_files,
            &glob_root,
        );
        tracing::debug!(
            backend = %backend_identifier,
            source = %source_checkout.pinned,
            glob_count = input_globs.len(),
            "computing metadata input hash",
        );
        let input_hash = Self::compute_input_hash(
            command_dispatcher,
            &source_checkout,
            &glob_root,
            input_globs,
            additional_glob_hash,
        )
        .await?;

        Ok(CachedCondaMetadata {
            id: random(),
            input_hash: input_hash.clone(),
            glob_root: Some(glob_root),
            metadata: MetadataKind::Outputs {
                outputs: outputs.outputs,
            },
        })
    }

    /// Computes the input hash for metadata returned by the backend.
    async fn compute_input_hash(
        command_queue: CommandDispatcher,
        source: &SourceCheckout,
        glob_root: &Path,
        input_globs: BTreeSet<String>,
        additional_glob_hash: Vec<u8>,
    ) -> Result<Option<InputHash>, CommandDispatcherError<BuildBackendMetadataError>> {
        if source.pinned.is_immutable() {
            // If the source is immutable (e.g., a git commit), we do not need to compute an
            // input hash because the contents of the source are fixed.
            return Ok(None);
        }

        // Compute the input hash based on the manifest path and the input globs.
        let input_hash = command_queue
            .glob_hash_cache()
            .compute_hash(GlobHashKey::new(
                glob_root.to_path_buf(),
                input_globs.clone(),
                additional_glob_hash,
            ))
            .await
            .map_err(BuildBackendMetadataError::GlobHash)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(Some(InputHash {
            hash: input_hash.hash,
            globs: input_globs,
        }))
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

/// Returns the input glob set extended with any variant file paths
/// relative to the source checkout root.
/// Paths are normalised to use forward slashes so that they are glob-compatible.
fn extend_input_globs_with_variant_files(
    mut input_globs: BTreeSet<String>,
    variant_files: &Option<Vec<PathBuf>>,
    glob_root: &Path,
) -> BTreeSet<String> {
    if let Some(variant_files) = variant_files {
        for variant_file in variant_files {
            let relative = match variant_file.strip_prefix(glob_root) {
                Ok(stripped) => stripped.to_path_buf(),
                Err(_) => {
                    diff_paths(variant_file, glob_root).unwrap_or_else(|| variant_file.clone())
                }
            };
            let glob = relative.to_string_lossy().replace("\\", "/");
            input_globs.insert(glob);
        }
    }
    input_globs
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildBackendMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(#[from] pixi_build_discovery::DiscoveryError),

    #[error("could not initialize the build-backend")]
    Initialize(
        #[diagnostic_source]
        #[from]
        InstantiateBackendError,
    ),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Communication(#[from] pixi_build_frontend::json_rpc::CommunicationError),

    #[error("the build backend {0} does not support the `conda/outputs` procedure")]
    BackendMissingCapabilities(String),

    #[error("could not compute hash of input files")]
    GlobHash(#[from] pixi_glob::GlobHashError),

    #[error(transparent)]
    Cache(#[from] source_metadata_cache::SourceMetadataCacheError),
}

/// Computes an additional hash to be used in glob hash
pub fn calculate_additional_glob_hash(
    project_model: &Option<ProjectModelV1>,
    variants: &Option<BTreeMap<String, Vec<String>>>,
) -> Vec<u8> {
    let mut hasher = Xxh3::new();
    if let Some(project_model) = project_model {
        project_model.hash(&mut hasher);
    }
    if let Some(variants) = variants {
        if !variants.is_empty() {
            variants.hash(&mut hasher);
        }
    }
    hasher.finish().to_ne_bytes().to_vec()
}
