use futures::{SinkExt, channel::mpsc::UnboundedSender};
use miette::Diagnostic;
use once_cell::sync::Lazy;
use pixi_build_discovery::CommandSpec;
use pixi_build_types::procedures::conda_outputs::CondaOutputsParams;
use pixi_glob::GlobSet;
use pixi_record::{CanonicalSourceLocation, PinnedBuildSourceSpec, PinnedSourceSpec, VariantValue};
use pixi_spec::{ResolvedExcludeNewer, SourceAnchor, SourceLocationSpec};
use rattler_conda_types::ChannelUrl;
use std::time::SystemTime;
use std::{
    collections::{BTreeMap, HashSet},
    hash::Hash,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use thiserror::Error;

use crate::build::CanonicalSourceCodeLocation;
use crate::cache::markers::BackendMetadataDir;
use crate::cache::{
    BuildBackendMetadataCache, BuildBackendMetadataCacheEntry, BuildBackendMetadataCacheError,
    BuildBackendMetadataCacheKey, CacheEntry, CacheKey, CacheKeyString, CacheRevision,
    MetadataCache, MetadataCacheKey, WriteResult,
};
use crate::compute_data::{HasBuildBackendMetadataCache, HasBuildBackendMetadataReporter};
use crate::injected_config::{BackendOverrideKey, EnabledProtocolsKey};
use crate::input_hash::{BackendSpecHash, ConfigurationHash, ProjectModelHash};
use crate::{
    BackendHandle, BuildEnvironment, EnvironmentRef, InstantiateBackendError,
    InstantiateBackendKey, ProjectModelOverrides, SourceCheckout, SourceCheckoutError,
    SourceCheckoutExt,
    build::{PinnedSourceCodeLocation, SourceRecordOrCheckout, WorkDirKey},
};
use pixi_build_discovery::BackendSpec;
use pixi_build_frontend::BackendOverride;
use pixi_compute_cache_dirs::CacheDirsExt;
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_path::normalize::normalize_typed;

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

/// Public request for build-backend metadata. The outer layer of the
/// two-level key design: identity is
/// `(manifest_source, preferred_build_source, env_ref)`, so two envs
/// with different ids don't dedup here even when their content is
/// identical. The actual backend compute lives on
/// [`BuildBackendMetadataInner`]; this outer Key's compute body
/// resolves `env_ref` via projections, builds an inner, and delegates.
/// Envs with equivalent resolved content converge at the inner layer
/// and share a single backend spawn.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct BuildBackendMetadataSpec {
    /// Manifest source. Passed through to the inner compute.
    pub manifest_source: PinnedSourceSpec,

    /// Optional pinned location of the source code; hint for the
    /// discovered backend. Passed through to the inner compute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_build_source: Option<PinnedSourceSpec>,

    /// Environment context that drives channels, build environment,
    /// variant configuration, and `exclude_newer`. Resolved via the
    /// dispatcher's [`WorkspaceEnvRegistry`](crate::WorkspaceEnvRegistry)
    /// at compute time.
    #[serde(skip)]
    pub env_ref: EnvironmentRef,

    /// User-supplied build string prefix forwarded to the backend's
    /// project model. Overrides any value declared in the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_string_prefix: Option<String>,

    /// User-supplied build number forwarded to the backend's project
    /// model. Overrides any value declared in the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_number: Option<u64>,
}

/// Compute-engine [`Key`] for the public-facing backend-metadata
/// request. Thin orchestrator that reads projections off `env_ref` to
/// build a [`BuildBackendMetadataInner`], then delegates to it via
/// `ctx.compute`. All actual work (and the
/// [`BuildBackendMetadataReporter`](crate::BuildBackendMetadataReporter)
/// lifecycle, read from the engine `DataStore`) lives on the inner
/// Key.
#[derive(Clone, Debug, Hash, Eq, PartialEq, derive_more::Display)]
#[display("{}", _0.manifest_source)]
pub struct BuildBackendMetadataKey(pub Arc<BuildBackendMetadataSpec>);

impl BuildBackendMetadataKey {
    pub fn new(spec: BuildBackendMetadataSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Key for BuildBackendMetadataKey {
    type Value = Result<Arc<BuildBackendMetadata>, BuildBackendMetadataError>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        // Resolve env_ref through the projection Keys so the dep graph
        // tracks the env fields this request depends on.
        let channels = ctx
            .compute(&crate::ChannelsOf(self.0.env_ref.clone()))
            .await;
        let build_environment = ctx
            .compute(&crate::BuildEnvOf(self.0.env_ref.clone()))
            .await;
        let variants = ctx
            .compute(&crate::VariantsOf(self.0.env_ref.clone()))
            .await;
        let exclude_newer = ctx
            .compute(&crate::ExcludeNewerOf(self.0.env_ref.clone()))
            .await;

        let inner = BuildBackendMetadataInner {
            manifest_source: self.0.manifest_source.clone(),
            preferred_build_source: self.0.preferred_build_source.clone(),
            channels: (*channels).clone(),
            build_environment: (*build_environment).clone(),
            variant_configuration: variants.variant_configuration.clone(),
            variant_files: variants.variant_files.clone(),
            exclude_newer: (*exclude_newer).clone(),
            build_string_prefix: self.0.build_string_prefix.clone(),
            build_number: self.0.build_number,
        };

        ctx.compute(&BuildBackendMetadataInnerKey::new(inner)).await
    }
}

/// Represents a request for metadata from a build backend for a particular
/// source location. The result of this request is the metadata for that
/// particular source.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct BuildBackendMetadataInner {
    /// The location that refers to where the manifest is stored.
    pub manifest_source: PinnedSourceSpec,

    /// The optional pinned location of the source code. If not provided, the
    /// location in the manifest is resolved.
    ///
    /// This is passed as a hint. If the [`pixi_spec::SourceSpec`] in the
    /// discovered manifest does not match with the pinned source provided
    /// here, the one in the manifest takes precedence and it is reresolved.
    ///
    /// See [`PinnedSourceSpec::matches_source_spec`] how the matching is done.
    pub preferred_build_source: Option<PinnedSourceSpec>,

    /// Exclude packages newer than the configured cutoffs when solving backend environments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<ResolvedExcludeNewer>,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about the build environment.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variant_configuration: BTreeMap<String, Vec<VariantValue>>,

    /// Variant file paths provided by the workspace.
    pub variant_files: Vec<PathBuf>,

    /// User-supplied build string prefix; overrides the manifest's
    /// project model when set. Part of the cache key so different
    /// overrides produce distinct backend invocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_string_prefix: Option<String>,

    /// User-supplied build number; overrides the manifest's project
    /// model when set. Part of the cache key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_number: Option<u64>,
}

/// The metadata of a source checkout.
#[derive(Debug)]
pub struct BuildBackendMetadata {
    /// The manifest and optional build source location for this metadata.
    pub source: PinnedSourceCodeLocation,

    /// The cache key string that was used to store/look up this metadata.
    pub cache_key: CacheKeyString<BuildBackendMetadataCache>,

    /// The metadata that was acquired from the build backend.
    pub metadata: CacheEntry<BuildBackendMetadataCache>,

    /// Whether caching should be skipped for this backend.
    ///
    /// This is true for System backends and path-based (mutable) backends
    /// which can change between runs.
    pub skip_cache: bool,
}

impl BuildBackendMetadataInner {
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

    /// Verifies if the cached metadata is still fresh.
    ///
    /// Returns:
    /// - `Ok(Ok(metadata))` if the cache is fresh and can be used as-is.
    /// - `Ok(Err(Some(metadata)))` if the cache is stale but the metadata is
    ///   returned for comparison (e.g. to reuse the ID if outputs match).
    /// - `Ok(Err(None))` if no cache entry exists.
    async fn verify_cache_freshness(
        cache_entry: Option<CacheEntry<BuildBackendMetadataCache>>,
        build_source_checkout: &SourceCheckout,
        project_model_hash: Option<ProjectModelHash>,
        configuration_hash: ConfigurationHash,
        backend_spec_hash: BackendSpecHash,
        requested_variants: &BTreeMap<String, Vec<VariantValue>>,
    ) -> Result<
        Result<
            CacheEntry<BuildBackendMetadataCache>,
            Option<CacheEntry<BuildBackendMetadataCache>>,
        >,
        BuildBackendMetadataError,
    > {
        let Some(cache_entry) = cache_entry else {
            return Ok(Err(None));
        };

        // Check the project model
        if cache_entry.project_model_hash != project_model_hash {
            tracing::info!(
                "found cached outputs with different project model, invalidating cache."
            );
            return Ok(Err(Some(cache_entry)));
        }

        // Check the build configuration
        if cache_entry.configuration_hash != configuration_hash {
            tracing::info!(
                "found cached outputs with different build configuration, invalidating cache."
            );
            return Ok(Err(Some(cache_entry)));
        }

        // Check the backend spec. Entries written before this field existed
        // have `None`; treat them as stale so they get repopulated with a
        // recorded spec hash.
        if cache_entry.backend_spec_hash != Some(backend_spec_hash) {
            tracing::info!(
                "found cached outputs with different backend specification, invalidating cache."
            );
            return Ok(Err(Some(cache_entry)));
        }

        // Check if the build variants match
        if &cache_entry.build_variants != requested_variants {
            tracing::info!("found cached outputs with different variants, invalidating cache.");
            return Ok(Err(Some(cache_entry)));
        }

        // If the build source is immutable, we don't check the contents of the files.
        if build_source_checkout.is_immutable() {
            return Ok(Ok(cache_entry));
        }

        let build_source_dir = build_source_checkout.path.as_dir_or_file_parent();

        // Check the files that were explicitly mentioned.
        for source_file_path in cache_entry
            .input_files
            .iter()
            .map(|path| build_source_dir.join(path).into_std_path_buf())
            .chain(cache_entry.build_variant_files.iter().cloned())
        {
            match source_file_path.metadata().and_then(|m| m.modified()) {
                Ok(modified_date) => {
                    if modified_date > cache_entry.timestamp {
                        tracing::info!(
                            "found cached outputs but '{}' has been modified, invalidating cache.",
                            source_file_path.display()
                        );
                        return Ok(Err(Some(cache_entry)));
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    tracing::info!(
                        "found cached outputs but '{}' has been deleted, invalidating cache.",
                        source_file_path.display()
                    );
                    return Ok(Err(Some(cache_entry)));
                }
                Err(err) => {
                    tracing::info!(
                        "found cached outputs but requested metadata for '{}' failed with: {}",
                        source_file_path.display(),
                        err
                    );
                    return Ok(Err(Some(cache_entry)));
                }
            };
        }

        let glob_set = GlobSet::create(cache_entry.input_globs.iter().map(String::as_str));
        for matching_file in glob_set
            .collect_matching(build_source_dir.as_std_path())
            .map_err(BuildBackendMetadataError::from)?
        {
            let path = matching_file.into_path();
            if cache_entry.input_files.contains(&path) {
                tracing::info!(
                    "found cached outputs but a new matching file at '{}' has been detected, invalidating cache.",
                    path.display()
                );
                return Ok(Err(Some(cache_entry)));
            }
        }

        Ok(Ok(cache_entry))
    }

    /// Validates that outputs with the same name have unique variants.
    #[allow(clippy::result_large_err)]
    fn validate_unique_variants(
        outputs: &[pixi_build_types::procedures::conda_outputs::CondaOutput],
    ) -> Result<(), BuildBackendMetadataError> {
        use std::collections::HashMap;

        // Group outputs by package name
        let mut outputs_by_name: HashMap<_, Vec<_>> = HashMap::new();
        for output in outputs {
            outputs_by_name
                .entry(&output.metadata.name)
                .or_default()
                .push(output);
        }

        // Check for duplicate variants within each package name group
        for (package_name, package_outputs) in outputs_by_name {
            if package_outputs.len() <= 1 {
                // No duplicates possible with 0 or 1 outputs
                continue;
            }

            let mut seen_variants = HashSet::new();
            let mut duplicate_variants = Vec::new();

            for output in package_outputs {
                let variant = &output.metadata.variant;
                if !seen_variants.insert(variant) {
                    // This variant was already seen, so it's a duplicate
                    duplicate_variants.push(format!("{variant:?}"));
                }
            }

            if !duplicate_variants.is_empty() {
                return Err(BuildBackendMetadataError::DuplicateVariants {
                    package: package_name.as_normalized().to_string(),
                    duplicates: duplicate_variants.join(", "),
                });
            }
        }

        Ok(())
    }

    /// Use the `conda/outputs` procedure to get the metadata for the source
    /// checkout.
    async fn call_conda_outputs(
        &self,
        metadata_dir: &pixi_path::AbsPresumedDirPath,
        build_source_checkout: &SourceCheckout,
        source_unique_key: &str,
        backend: BackendHandle,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<RawCondaOutputs, BuildBackendMetadataError> {
        let backend_guard = backend.lock().await;
        let backend_identifier = backend_guard.identifier().to_string();
        let params = CondaOutputsParams {
            channels: self.channels.clone(),
            host_platform: self.build_environment.host_platform,
            build_platform: self.build_environment.build_platform,
            variant_configuration: Some(
                self.variant_configuration
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.iter()
                                .cloned()
                                .map(pixi_build_types::VariantValue::from)
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            variant_files: Some(self.variant_files.clone()),
            // Work dir nests under the per-source slot of the metadata
            // cache, so cache entries for this source and the backend's
            // scratch for the same source live side by side (single
            // `rm -rf` cleans both).
            work_directory: metadata_dir
                .join(source_unique_key)
                .join(pixi_consts::consts::BACKEND_METADATA_WORK_SUBDIR)
                .into_assume_dir()
                .join(
                    WorkDirKey {
                        source: SourceRecordOrCheckout::Checkout {
                            checkout: build_source_checkout.clone(),
                        },
                        host_platform: self.build_environment.host_platform,
                        build_backend: backend_identifier.clone(),
                    }
                    .key(),
                )
                .into_std_path_buf(),
        };
        let outputs = backend_guard
            .conda_outputs(params, move |line| {
                let _err = futures::executor::block_on(log_sink.send(line));
            })
            .await
            .map_err(|e| BuildBackendMetadataError::Communication(Arc::new(e)))?;
        let timestamp = SystemTime::now();

        // If the backend supports unique variants, validate that outputs with the same name
        // have unique variants
        if backend_guard.api_version().supports_unique_variants() {
            Self::validate_unique_variants(&outputs.outputs)?;
        }

        for output in &outputs.outputs {
            tracing::debug!(
                backend = %backend_identifier,
                package = %output.metadata.name.as_source(),
                version = %output.metadata.version,
                build = %output.metadata.build,
                subdir = %output.metadata.subdir,
                "received metadata output from backend",
            );
        }

        // Determine the files that match the input globs.
        let globs_root = build_source_checkout.path.as_dir_or_file_parent();
        let input_glob_set = GlobSet::create(outputs.input_globs.iter().map(String::as_str));
        let globs_root_path = globs_root.as_std_path();
        let input_glob_files = input_glob_set
            .collect_matching(globs_root_path)
            .map_err(BuildBackendMetadataError::from)?
            .into_iter()
            .map(|entry| {
                let path = entry.into_path();
                path.strip_prefix(globs_root_path)
                    .ok()
                    .unwrap_or(&path)
                    .to_path_buf()
            })
            .collect();

        Ok(RawCondaOutputs {
            outputs: outputs.outputs,
            input_globs: outputs.input_globs.into_iter().collect(),
            input_files: input_glob_files,
            timestamp,
        })
    }
}

/// Compute-engine [`Key`] for the content-hashed backend-metadata
/// compute. Dedup is structural: two requests for the same
/// [`BuildBackendMetadataInner`] share a single compute, which lets
/// multiple workspace envs that converge on identical content share
/// one backend spawn. The disk cache sits underneath, reached via
/// [`DataStore`](pixi_compute_engine::DataStore) through
/// [`HasBuildBackendMetadataCache`].
#[derive(Clone, Debug, Hash, Eq, PartialEq, derive_more::Display)]
#[display("{}", _0.manifest_source)]
pub(crate) struct BuildBackendMetadataInnerKey(pub(crate) Arc<BuildBackendMetadataInner>);

impl BuildBackendMetadataInnerKey {
    pub(crate) fn new(inner: BuildBackendMetadataInner) -> Self {
        Self(Arc::new(inner))
    }
}

impl Key for BuildBackendMetadataInnerKey {
    type Value = Result<Arc<BuildBackendMetadata>, BuildBackendMetadataError>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        // Reporter lifecycle: queue up-front so the reporter can count
        // us before any real work starts. The `on_started` event carries
        // the log-output receiver so the reporter can stream backend
        // output as it arrives.
        let reporter_arc = ctx.global_data().build_backend_metadata_reporter().cloned();
        let reporter_id = reporter_arc.as_deref().map(|r| r.on_queued(&self.0));

        let (log_sink, log_rx) = futures::channel::mpsc::unbounded::<String>();
        if let (Some(r), Some(id)) = (reporter_arc.as_deref(), reporter_id) {
            r.on_started(id, Box::new(log_rx));
        }

        // Scope nested Keys under this metadata request's id.
        let work = self.0.clone().compute_inner(ctx, log_sink);
        let result = match reporter_id {
            Some(id) => id.scope_active(work).await,
            None => work.await,
        };

        if let (Some(r), Some(id)) = (reporter_arc.as_deref(), reporter_id) {
            r.on_finished(id, result.is_err());
        }

        result.map(Arc::new)
    }
}

/// Checked-out sources plus the discovered backend, threaded through
/// the compute pipeline.
struct ResolvedCheckouts {
    /// Checkout containing the manifest. Always populated.
    manifest_source_checkout: SourceCheckout,
    /// Checkout of the source to build from. Equals
    /// `manifest_source_checkout` when the backend does not declare an
    /// out-of-tree build source.
    build_source_checkout: SourceCheckout,
    /// Wrapper describing how the build source relates to the manifest
    /// source (relative subdirectory vs. absolute pinned spec). `None`
    /// when the build source is the manifest source.
    build_source: Option<PinnedBuildSourceSpec>,
    /// Anchor used to resolve relative references in the discovered
    /// backend spec.
    manifest_source_anchor: SourceAnchor,
    /// Backend discovered from the manifest checkout.
    discovered_backend: Arc<pixi_build_discovery::DiscoveredBackend>,
}

impl ResolvedCheckouts {
    /// The canonical source location stored in cache keys and
    /// returned in [`BuildBackendMetadata::source`].
    fn manifest_source_location(&self) -> PinnedSourceCodeLocation {
        PinnedSourceCodeLocation::new(
            self.manifest_source_checkout.pinned.clone(),
            self.build_source.clone(),
        )
    }
}

/// Outcome of probing the on-disk metadata cache.
enum CacheProbe {
    /// Cache contained a fresh entry; the caller can return it
    /// directly without contacting the backend.
    Hit(BuildBackendMetadata),
    /// Cache missed or was stale; the caller must call the backend and
    /// write a new entry. `stale` carries any prior entry so the caller
    /// can reuse the revision when outputs are unchanged, and bump the
    /// cache version on conflict.
    Miss {
        cache_key: CacheKey<BuildBackendMetadataCache>,
        stale: Option<CacheEntry<BuildBackendMetadataCache>>,
        project_model_hash: Option<ProjectModelHash>,
        configuration_hash: ConfigurationHash,
        backend_spec_hash: BackendSpecHash,
        skip_cache: bool,
    },
}

impl BuildBackendMetadataInner {
    /// Bundle the user-supplied project-model overrides into the shape
    /// expected by [`InstantiateBackendKey`]. Backend instantiation is
    /// content-addressed on these overrides, so callers with the same
    /// values share a single backend handle.
    fn project_model_overrides(&self) -> ProjectModelOverrides {
        ProjectModelOverrides {
            build_string_prefix: self.build_string_prefix.clone(),
            build_number: self.build_number,
        }
    }

    /// Resolve the manifest and build-source checkouts and the backend
    /// that owns them.
    async fn resolve_checkouts(
        &self,
        ctx: &mut ComputeCtx,
    ) -> Result<ResolvedCheckouts, BuildBackendMetadataError> {
        let manifest_source_checkout = ctx
            .checkout_pinned_source(self.manifest_source.clone())
            .await?;

        let discovered_backend = ctx
            .compute(&crate::DiscoveredBackendKey::new(
                manifest_source_checkout.path.as_std_path(),
            ))
            .await
            .map_err(BuildBackendMetadataError::Discovery)?;

        let manifest_source_anchor =
            SourceAnchor::from(SourceLocationSpec::from(self.manifest_source.clone()));

        let build_source_checkout_and_spec = match &discovered_backend.init_params.build_source {
            None => None,
            Some(build_source) => {
                let relative_build_source_spec = if let SourceLocationSpec::Path(path) =
                    build_source
                    && path.path.is_relative()
                {
                    Some(normalize_typed(path.path.to_path()).to_string())
                } else {
                    None
                };

                let resolved_location = manifest_source_anchor.resolve(build_source.clone());

                let checkout = match &self.preferred_build_source {
                    Some(pinned) if pinned.matches_source_spec(&resolved_location) => {
                        ctx.checkout_pinned_source(pinned.clone()).await?
                    }
                    _ => ctx.pin_and_checkout(resolved_location).await?,
                };
                Some((checkout, relative_build_source_spec))
            }
        };

        let (build_source_checkout, build_source) = match build_source_checkout_and_spec {
            Some((checkout, relative_build_source)) => {
                let pinned = checkout.pinned.clone();
                let spec = if let Some(relative) = relative_build_source {
                    PinnedBuildSourceSpec::Relative(relative, pinned)
                } else {
                    PinnedBuildSourceSpec::Absolute(pinned)
                };
                (checkout, Some(spec))
            }
            None => (manifest_source_checkout.clone(), None),
        };

        Ok(ResolvedCheckouts {
            manifest_source_checkout,
            build_source_checkout,
            build_source,
            manifest_source_anchor,
            discovered_backend,
        })
    }

    /// Read the metadata cache and decide whether we can short-circuit.
    ///
    /// Returns [`CacheProbe::Hit`] on a fresh cache hit, or
    /// [`CacheProbe::Miss`] carrying every input the caller needs to
    /// call the backend and write a new entry.
    async fn probe_cache(
        &self,
        ctx: &mut ComputeCtx,
        checkouts: &ResolvedCheckouts,
    ) -> Result<CacheProbe, BuildBackendMetadataError> {
        let enabled_protocols = ctx.compute(&EnabledProtocolsKey).await;
        let backend_override = ctx.compute(&BackendOverrideKey).await;
        let skip_cache = Self::should_skip_metadata_cache(
            &checkouts.discovered_backend.backend_spec,
            &backend_override,
        );

        let manifest_source_location = checkouts.manifest_source_location();
        let cache = ctx.global_data().build_backend_metadata_cache();
        let cache_key: CacheKey<BuildBackendMetadataCache> = BuildBackendMetadataCacheKey {
            channel_urls: self.channels.clone(),
            build_environment: self.build_environment.clone(),
            exclude_newer: self.exclude_newer.clone(),
            enabled_protocols: enabled_protocols.as_ref().clone(),
            source: manifest_source_location.clone().into(),
        };
        let cache_read_result = cache
            .read(&cache_key)
            .await
            .map_err(BuildBackendMetadataError::Cache)?;

        // Apply CLI overrides before hashing the project model so the
        // cache distinguishes builds invoked with different overrides.
        let overrides = self.project_model_overrides();
        let overridden_project_model = overrides.apply(
            checkouts
                .discovered_backend
                .init_params
                .project_model
                .clone(),
        );
        let project_model_hash = overridden_project_model
            .as_ref()
            .map(ProjectModelHash::from);
        let configuration_hash = ConfigurationHash::compute(
            checkouts
                .discovered_backend
                .init_params
                .configuration
                .as_ref(),
            checkouts
                .discovered_backend
                .init_params
                .target_configuration
                .as_ref(),
        );
        let backend_spec_hash =
            BackendSpecHash::from(&checkouts.discovered_backend.backend_spec);

        if skip_cache {
            let BackendSpec::JsonRpc(spec) = &checkouts.discovered_backend.backend_spec;
            warn_once_per_backend(&spec.name);
            return Ok(CacheProbe::Miss {
                cache_key,
                stale: None,
                project_model_hash,
                configuration_hash,
                backend_spec_hash,
                skip_cache,
            });
        }

        match Self::verify_cache_freshness(
            cache_read_result,
            &checkouts.build_source_checkout,
            project_model_hash,
            configuration_hash,
            backend_spec_hash,
            &self.variant_configuration,
        )
        .await?
        {
            Ok(fresh) => {
                tracing::debug!("Using cached build backend metadata");
                Ok(CacheProbe::Hit(BuildBackendMetadata {
                    source: manifest_source_location,
                    cache_key: cache_key.key(),
                    metadata: fresh,
                    skip_cache,
                }))
            }
            Err(stale) => Ok(CacheProbe::Miss {
                cache_key,
                stale,
                project_model_hash,
                configuration_hash,
                backend_spec_hash,
                skip_cache,
            }),
        }
    }

    /// Orchestrates the four phases of producing
    /// [`BuildBackendMetadata`]: checkouts → cache probe → backend
    /// invocation → cache write.
    async fn compute_inner(
        self: Arc<Self>,
        ctx: &mut ComputeCtx,
        log_sink: UnboundedSender<String>,
    ) -> Result<BuildBackendMetadata, BuildBackendMetadataError> {
        let checkouts = self.resolve_checkouts(ctx).await?;

        let (
            cache_key,
            stale,
            project_model_hash,
            configuration_hash,
            backend_spec_hash,
            skip_cache,
        ) = match self.probe_cache(ctx, &checkouts).await? {
            CacheProbe::Hit(metadata) => return Ok(metadata),
            CacheProbe::Miss {
                cache_key,
                stale,
                project_model_hash,
                configuration_hash,
                backend_spec_hash,
                skip_cache,
            } => (
                cache_key,
                stale,
                project_model_hash,
                configuration_hash,
                backend_spec_hash,
                skip_cache,
            ),
        };

        // Instantiate the backend. `DiscoveredBackendKey` dedups inside
        // the key, so the re-discovery is free.
        let backend = ctx
            .compute(
                &InstantiateBackendKey::new(
                    checkouts.manifest_source_checkout.path.as_std_path(),
                    checkouts.manifest_source_anchor.clone(),
                    checkouts
                        .build_source_checkout
                        .path
                        .as_dir_or_file_parent()
                        .to_path_buf(),
                    self.exclude_newer.clone(),
                )
                .with_project_model_overrides(self.project_model_overrides()),
            )
            .await
            .map_err(|e: Arc<InstantiateBackendError>| {
                BuildBackendMetadataError::Initialize((*e).clone())
            })?;

        {
            let guard = backend.lock().await;
            if !guard.capabilities().provides_conda_outputs() {
                return Err(BuildBackendMetadataError::BackendMissingCapabilities(
                    guard.identifier().to_string(),
                ));
            }
        }

        tracing::trace!(
            "Using `{}` procedure to get metadata information",
            pixi_build_types::procedures::conda_outputs::METHOD_NAME
        );

        // Resolve the metadata cache root once for the conda_outputs
        // call below so the work-dir path derivation does not duplicate
        // the dependency edge to `BackendMetadataDir`.
        let metadata_dir = ctx.cache_dir::<BackendMetadataDir>().await;

        // Compute the source's cache_unique_key up front: the work dir
        // is nested under the same `<source>/` slot the metadata cache
        // uses, so both live in one tree per source.
        let manifest_source_location = checkouts.manifest_source_location();
        let canonical_manifest_source: CanonicalSourceLocation =
            manifest_source_location.manifest_source().into();
        let canonical_build_source =
            CanonicalSourceLocation::from(checkouts.build_source_checkout.pinned.clone());
        let canonical_build_source_opt = (canonical_manifest_source != canonical_build_source)
            .then_some(canonical_build_source.clone());
        let canonical_source = CanonicalSourceCodeLocation::new(
            canonical_manifest_source.clone(),
            canonical_build_source_opt.clone(),
        );
        let source_unique_key = canonical_source.cache_unique_key();

        let raw = self
            .call_conda_outputs(
                &metadata_dir,
                &checkouts.build_source_checkout,
                &source_unique_key,
                backend,
                log_sink,
            )
            .await?;

        // Reuse the previous revision when outputs are unchanged, so
        // downstream caches keyed by the revision remain valid.
        let revision = match &stale {
            Some(prev) if prev.outputs == raw.outputs => prev.revision.clone(),
            _ => CacheRevision::new(),
        };

        let prev_cache_version = stale.as_ref().map(|cache| cache.cache_version);
        let metadata = BuildBackendMetadataCacheEntry {
            revision,
            cache_version: prev_cache_version.map_or(0, |version| version + 1),
            outputs: raw.outputs,
            build_variants: self.variant_configuration.clone(),
            build_variant_files: self.variant_files.iter().cloned().collect(),
            input_globs: raw.input_globs,
            input_files: raw.input_files,
            source: canonical_source,
            project_model_hash,
            configuration_hash,
            backend_spec_hash: Some(backend_spec_hash),
            timestamp: raw.timestamp,
        };

        let cache = ctx.global_data().build_backend_metadata_cache();
        match cache
            .try_write(
                &cache_key,
                metadata.clone(),
                prev_cache_version.unwrap_or(0),
            )
            .await
            .map_err(BuildBackendMetadataError::Cache)?
        {
            WriteResult::Written => {
                tracing::trace!("Cache updated successfully");
            }
            WriteResult::Conflict(_other_metadata) => {
                tracing::debug!(
                    "Cache was updated by another process during computation (version conflict), using our computed result"
                );
            }
        }

        Ok(BuildBackendMetadata {
            source: manifest_source_location,
            cache_key: cache_key.key(),
            metadata,
            skip_cache,
        })
    }
}

/// Raw result from calling the build backend's `conda/outputs` procedure.
/// This contains only what the backend returns plus derived file information.
/// The caller is responsible for constructing the full `BuildBackendMetadataCacheEntry`.
struct RawCondaOutputs {
    /// The outputs as reported by the build backend.
    outputs: Vec<pixi_build_types::procedures::conda_outputs::CondaOutput>,
    /// Globs of files from which the metadata was derived.
    input_globs: std::collections::BinaryHeap<String>,
    /// Paths of files that match the input globs.
    input_files: std::collections::BTreeSet<PathBuf>,
    /// The timestamp of when the metadata was computed.
    timestamp: SystemTime,
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum BuildBackendMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(Arc<pixi_build_discovery::DiscoveryError>),

    #[error("could not initialize the build-backend")]
    Initialize(
        #[diagnostic_source]
        #[source]
        InstantiateBackendError,
    ),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Communication(Arc<pixi_build_frontend::json_rpc::CommunicationError>),

    #[error("the build backend {0} does not support the `conda/outputs` procedure")]
    BackendMissingCapabilities(String),

    #[error(
        "the build backend returned outputs with duplicate variants for package '{package}': {duplicates}"
    )]
    DuplicateVariants { package: String, duplicates: String },

    #[error("could not compute hash of input files")]
    GlobHash(Arc<pixi_glob::GlobHashError>),

    #[error("failed to determine input file modification times")]
    GlobSet(Arc<pixi_glob::GlobSetError>),

    #[error(transparent)]
    Cache(#[from] BuildBackendMetadataCacheError),

    #[error("failed to normalize path")]
    NormalizePath(Arc<pixi_path::NormalizeError>),
}

impl From<pixi_build_discovery::DiscoveryError> for BuildBackendMetadataError {
    fn from(err: pixi_build_discovery::DiscoveryError) -> Self {
        Self::Discovery(Arc::new(err))
    }
}

impl From<pixi_build_frontend::json_rpc::CommunicationError> for BuildBackendMetadataError {
    fn from(err: pixi_build_frontend::json_rpc::CommunicationError) -> Self {
        Self::Communication(Arc::new(err))
    }
}

impl From<pixi_glob::GlobHashError> for BuildBackendMetadataError {
    fn from(err: pixi_glob::GlobHashError) -> Self {
        Self::GlobHash(Arc::new(err))
    }
}

impl From<pixi_glob::GlobSetError> for BuildBackendMetadataError {
    fn from(err: pixi_glob::GlobSetError) -> Self {
        Self::GlobSet(Arc::new(err))
    }
}

impl From<pixi_path::NormalizeError> for BuildBackendMetadataError {
    fn from(err: pixi_path::NormalizeError) -> Self {
        Self::NormalizePath(Arc::new(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_build_types::VariantValue;
    use pixi_build_types::procedures::conda_outputs::{
        CondaOutput, CondaOutputDependencies, CondaOutputIgnoreRunExports, CondaOutputMetadata,
        CondaOutputRunExports,
    };
    use rattler_conda_types::{NoArchType, PackageName, Platform, Version};
    use std::collections::BTreeMap;

    fn create_test_output(name: &str, variant: BTreeMap<String, VariantValue>) -> CondaOutput {
        CondaOutput {
            metadata: CondaOutputMetadata {
                name: PackageName::try_from(name).unwrap(),
                version: Version::major(1).into(),
                build: "0".to_string(),
                build_number: 0,
                subdir: Platform::NoArch,
                license: None,
                license_family: None,
                noarch: NoArchType::none(),
                purls: None,
                python_site_packages_path: None,
                variant,
            },
            build_dependencies: None,
            host_dependencies: None,
            run_dependencies: CondaOutputDependencies {
                depends: vec![],
                constraints: vec![],
            },
            ignore_run_exports: CondaOutputIgnoreRunExports::default(),
            run_exports: CondaOutputRunExports::default(),
            input_globs: None,
        }
    }

    #[test]
    fn test_validate_unique_variants_with_unique_variants() {
        // Test case: outputs with the same name but different variants should pass
        let outputs = vec![
            create_test_output(
                "mypackage",
                BTreeMap::from([("python".to_string(), VariantValue::from("3.11"))]),
            ),
            create_test_output(
                "mypackage",
                BTreeMap::from([("python".to_string(), VariantValue::from("3.12"))]),
            ),
        ];

        let result = BuildBackendMetadataInner::validate_unique_variants(&outputs);
        assert!(
            result.is_ok(),
            "Expected validation to pass for unique variants"
        );
    }

    #[test]
    fn test_validate_unique_variants_with_duplicate_variants() {
        // Test case: outputs with the same name and same variants should fail
        let outputs = vec![
            create_test_output(
                "mypackage",
                BTreeMap::from([("python".to_string(), VariantValue::from("3.11"))]),
            ),
            create_test_output(
                "mypackage",
                BTreeMap::from([("python".to_string(), VariantValue::from("3.11"))]),
            ),
        ];

        let result = BuildBackendMetadataInner::validate_unique_variants(&outputs);
        assert!(
            result.is_err(),
            "Expected validation to fail for duplicate variants"
        );

        if let Err(BuildBackendMetadataError::DuplicateVariants {
            package,
            duplicates,
        }) = result
        {
            assert_eq!(package, "mypackage");
            assert!(duplicates.contains("python"));
        } else {
            panic!("Expected DuplicateVariants error");
        }
    }

    #[test]
    fn test_validate_unique_variants_with_empty_variants() {
        // Test case: outputs with the same name and empty variants should fail
        let outputs = vec![
            create_test_output("mypackage", BTreeMap::new()),
            create_test_output("mypackage", BTreeMap::new()),
        ];

        let result = BuildBackendMetadataInner::validate_unique_variants(&outputs);
        assert!(
            result.is_err(),
            "Expected validation to fail for duplicate empty variants"
        );
    }

    #[test]
    fn test_validate_unique_variants_with_different_packages() {
        // Test case: outputs with different names can have the same variants
        let outputs = vec![
            create_test_output(
                "package-a",
                BTreeMap::from([("python".to_string(), VariantValue::from("3.11"))]),
            ),
            create_test_output(
                "package-b",
                BTreeMap::from([("python".to_string(), VariantValue::from("3.11"))]),
            ),
        ];

        let result = BuildBackendMetadataInner::validate_unique_variants(&outputs);
        assert!(
            result.is_ok(),
            "Expected validation to pass for different packages with same variants"
        );
    }

    #[test]
    fn test_validate_unique_variants_with_single_output() {
        // Test case: a single output should always pass
        let outputs = vec![create_test_output(
            "mypackage",
            BTreeMap::from([("python".to_string(), VariantValue::from("3.11"))]),
        )];

        let result = BuildBackendMetadataInner::validate_unique_variants(&outputs);
        assert!(
            result.is_ok(),
            "Expected validation to pass for single output"
        );
    }

    #[test]
    fn test_validate_unique_variants_with_multiple_variant_keys() {
        // Test case: outputs with multiple variant keys, one duplicate
        let outputs = vec![
            create_test_output(
                "mypackage",
                BTreeMap::from([
                    ("python".to_string(), VariantValue::from("3.11")),
                    ("cuda".to_string(), VariantValue::from("11.8")),
                ]),
            ),
            create_test_output(
                "mypackage",
                BTreeMap::from([
                    ("python".to_string(), VariantValue::from("3.11")),
                    ("cuda".to_string(), VariantValue::from("12.0")),
                ]),
            ),
            create_test_output(
                "mypackage",
                BTreeMap::from([
                    ("python".to_string(), VariantValue::from("3.11")),
                    ("cuda".to_string(), VariantValue::from("11.8")),
                ]),
            ),
        ];

        let result = BuildBackendMetadataInner::validate_unique_variants(&outputs);
        assert!(
            result.is_err(),
            "Expected validation to fail for duplicate multi-key variants"
        );
    }
}
