use futures::{SinkExt, channel::mpsc::UnboundedSender};
use itertools::{Either, Itertools};
use miette::Diagnostic;
use once_cell::sync::Lazy;
use pathdiff::diff_paths;
use pixi_build_discovery::{CommandSpec, EnabledProtocols};
use pixi_build_frontend::Backend;
use pixi_build_types::procedures::conda_outputs::CondaOutputsParams;
use pixi_glob::{GlobSet, GlobSetError};
use pixi_record::{PinnedSourceSpec, VariantValue};
use pixi_spec::{SourceAnchor, SourceSpec};
use rattler_conda_types::{ChannelConfig, ChannelUrl};
use std::time::SystemTime;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    hash::Hash,
    path::PathBuf,
    sync::Mutex,
};
use thiserror::Error;
use tracing::instrument;

use crate::cache::build_backend_metadata::CachedCondaMetadataId;
use crate::input_hash::ProjectModelHash;
use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    InstantiateBackendError, InstantiateBackendSpec, SourceCheckout, SourceCheckoutError,
    build::{SourceCodeLocation, SourceRecordOrCheckout, WorkDirKey},
    cache::{
        build_backend_metadata::{self, BuildBackendMetadataCacheShard, CachedCondaMetadata},
        common::MetadataCache,
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
    /// The location that refers to where the manifest is stored.
    pub manifest_source: PinnedSourceSpec,

    /// The optional pinned location of the source code. If not provided, the
    /// location in the manifest is resolved.
    ///
    /// This is passed as a hint. If the [`SourceSpec`] in the discovered
    /// manifest does not match with the pinned source provided here, the one
    /// in the manifest takes precedence and it is reresolved.
    ///
    /// See [`PinnedSourceSpec::matches_source_spec`] how the matching is done.
    pub preferred_build_source: Option<PinnedSourceSpec>,

    /// The channel configuration to use for the build backend.
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about the build environment.
    pub build_environment: BuildEnvironment,

    /// Variant configuration
    pub variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,

    /// Variant file paths provided by the workspace.
    pub variant_files: Option<Vec<PathBuf>>,

    /// The protocols that are enabled for this source
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

/// The metadata of a source checkout.
#[derive(Debug)]
pub struct BuildBackendMetadata {
    /// The manifest and optional build source location for this metadata.
    pub source: SourceCodeLocation,

    /// The metadata that was acquired from the build backend.
    pub metadata: CachedCondaMetadata,

    /// Whether caching should be skipped for this backend.
    ///
    /// This is true for System backends and path-based (mutable) backends
    /// which can change between runs.
    pub skip_cache: bool,
}

impl BuildBackendMetadataSpec {
    #[instrument(
        skip_all,
        name = "backend-metadata",
        fields(
            manifest_source = %self.manifest_source,
            build_source = self.preferred_build_source.as_ref().map(tracing::field::display),
            platform = %self.build_environment.host_platform,
        )
    )]
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
        log_sink: UnboundedSender<String>,
    ) -> Result<BuildBackendMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        // Ensure that the source is checked out before proceeding.
        let manifest_source_checkout = command_dispatcher
            // Never has an alternative root because we want to get the manifest
            .checkout_pinned_source(self.manifest_source.clone())
            .await
            .map_err_with(BuildBackendMetadataError::SourceCheckout)?;

        // Discover information about the build backend from the source code (cached by path).
        let discovered_backend = command_dispatcher
            .discover_backend(
                &manifest_source_checkout.path,
                self.channel_config.clone(),
                self.enabled_protocols.clone(),
            )
            .await
            .map_err_with(BuildBackendMetadataError::Discovery)?;

        let project_model_hash = discovered_backend
            .init_params
            .project_model
            .as_ref()
            .map(ProjectModelHash::from);

        // Determine the location of the source to build from.
        let manifest_source_anchor =
            SourceAnchor::from(SourceSpec::from(self.manifest_source.clone()));
        // `build_source` is still relative to the `manifest_source`
        let build_source_checkout = match &discovered_backend.init_params.build_source {
            None => None,
            Some(build_source) => {
                // An out of tree source is provided. Resolve it against the manifest source.
                let resolved_location = manifest_source_anchor.resolve(build_source.clone());
                let resolved_source_build_spec = SourceSpec {
                    location: resolved_location.clone(),
                };

                // Check if we have a preferred build source that matches this same location
                match &self.preferred_build_source {
                    Some(pinned) if pinned.matches_source_spec(&resolved_source_build_spec) => {
                        Some(
                            command_dispatcher
                                .checkout_pinned_source(pinned.clone())
                                .await
                                .map_err_with(BuildBackendMetadataError::SourceCheckout)?,
                        )
                    }
                    _ => Some(
                        command_dispatcher
                            .pin_and_checkout(resolved_location)
                            .await
                            .map_err_with(BuildBackendMetadataError::SourceCheckout)?,
                    ),
                }
            }
        };

        let (build_source_checkout, build_source) = if let Some(checkout) = build_source_checkout {
            let pinned = checkout.pinned.clone();
            (checkout, Some(pinned))
        } else {
            (manifest_source_checkout.clone(), None)
        };
        let source_location = SourceCodeLocation::new(
            manifest_source_checkout.pinned.clone(),
            build_source.clone(),
        );

        // Check if we should skip the metadata cache for this backend
        let skip_cache = Self::should_skip_metadata_cache(
            &discovered_backend.backend_spec,
            command_dispatcher.build_backend_overrides(),
        );

        // Check the source metadata cache, short circuit if there is a cache hit that
        // is still fresh.
        let cache_key = self.cache_shard();
        let cache_read_result = command_dispatcher
            .build_backend_metadata_cache()
            .read(&cache_key)
            .await
            .map_err(BuildBackendMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        let (cached_metadata, cache_version) = match cache_read_result {
            Some((metadata, version)) => (Some(metadata), version),
            // Start at cache version 0 if no cache exists
            None => (None, 0),
        };

        if !skip_cache {
            if let Some(cache_entry) = Self::verify_cache_freshness(
                cached_metadata,
                &build_source_checkout,
                project_model_hash,
                &self.variant_configuration,
            )
            .await?
            {
                return Ok(BuildBackendMetadata {
                    source: source_location.clone(),
                    metadata: cache_entry,
                    skip_cache,
                });
            }
        } else {
            let backend_name = match &discovered_backend.backend_spec {
                BackendSpec::JsonRpc(spec) => &spec.name,
            };
            warn_once_per_backend(backend_name);
        }

        let build_source_dir = build_source_checkout.path.clone();
        // Instantiate the backend with the discovered information.
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend
                    .backend_spec
                    .clone()
                    .resolve(manifest_source_anchor),
                build_source_dir,
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols.clone(),
                workspace_root: discovered_backend.init_params.workspace_root.clone(),
                manifest_path: discovered_backend.init_params.manifest_path.clone(),
                project_model: discovered_backend.init_params.project_model.clone(),
                configuration: discovered_backend.init_params.configuration.clone(),
                target_configuration: discovered_backend.init_params.target_configuration.clone(),
            })
            .await
            .map_err_with(BuildBackendMetadataError::Initialize)?;

        // Call the conda_outputs method to get metadata.
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
        let mut metadata = self
            .call_conda_outputs(
                command_dispatcher.clone(),
                build_source_checkout,
                project_model_hash,
                backend,
                log_sink,
            )
            .await?;

        metadata.cache_version = cache_version;

        // Try to store the metadata in the cache with version checking.
        // If another process updated the cache while we were computing, we get a conflict.
        match command_dispatcher
            .build_backend_metadata_cache()
            .try_write(&cache_key, metadata.clone(), cache_version)
            .await
            .map_err(BuildBackendMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?
        {
            build_backend_metadata::WriteResult::Written => {
                tracing::trace!("Cache updated successfully");
            }
            build_backend_metadata::WriteResult::Conflict(_other_metadata) => {
                // Another process computed and cached metadata while we were computing.
                // We use our computed result.
                tracing::debug!(
                    "Cache was updated by another process during computation (version conflict), using our computed result"
                );
            }
        }

        Ok(BuildBackendMetadata {
            source: source_location,
            metadata,
            skip_cache,
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
        cache_entry: Option<CachedCondaMetadata>,
        source_checkout: &SourceCheckout,
        project_model_hash: Option<ProjectModelHash>,
        requested_variants: &Option<BTreeMap<String, Vec<VariantValue>>>,
    ) -> Result<Option<CachedCondaMetadata>, CommandDispatcherError<BuildBackendMetadataError>>
    {
        let Some(cache_entry) = cache_entry else {
            return Ok(None);
        };

        // Check the project model
        if cache_entry.project_model_hash != project_model_hash {
            tracing::trace!(
                "found cached outputs with different project model, invalidating cache."
            );
            return Ok(None);
        }

        // Check if the source location changed.
        if cache_entry.build_source != source_checkout.pinned {
            tracing::trace!(
                "found cached outputs with different source code location, invalidating cache."
            );
            return Ok(None);
        }

        // Check if the build variants match
        if Some(&cache_entry.build_variants) != requested_variants.as_ref() {
            tracing::trace!("found cached outputs with different variants, invalidating cache.");
            return Ok(None);
        }

        // If the build source is immutable, we don't check the contents of the files.
        if source_checkout.is_immutable() {
            return Ok(Some(cache_entry));
        }

        // Determine the root
        let globs_root = &source_checkout.path;
        let globs = &cache_entry.input_globs;
        let walk_globs = || {
            let globs_root = if globs_root.is_dir() {
                // use the parent directory if the check is a file.
                globs_root.parent().expect("a dir must have a parent")
            } else {
                globs_root
            };
            let glob_set = GlobSet::create(globs.iter().map(|p| p.as_str()));
            match glob_set.collect_matching(globs_root) {
                Ok(matches) => Either::Left(matches.into_iter().map(|entry| {
                    entry
                        .metadata()
                        .map_err(|err| GlobSetError::Walk(entry.path().to_path_buf(), err))
                        .map(|m| (m, entry.path().to_path_buf()))
                })),
                Err(err) => Either::Right(std::iter::once(Err(err))),
            }
        };

        let walk_files = || {
            cache_entry.input_files.iter().map(|path| {
                let source_file_path = source_checkout.path.join(path);
                fs_err::metadata(&source_file_path)
                    .map_err(|err| GlobSetError::Walk(path.clone(), err.into()))
                    .map(|m| (m, path.clone()))
            })
        };

        // Check the files that were explicitly mentioned.
        for metadata in walk_files().chain(std::iter::once_with(walk_globs).flatten()) {
            let (metadata, path) = match metadata {
                Ok(metadata) => metadata,
                Err(GlobSetError::Walk(path, err))
                    if err.io_error().map(std::io::Error::kind)
                        == Some(std::io::ErrorKind::NotFound) =>
                {
                    tracing::trace!(
                        "found cached outputs but '{}' has been deleted, invalidating cache.",
                        path.display()
                    );
                    return Ok(None);
                }
                Err(err) => {
                    return Err(CommandDispatcherError::Failed(
                        BuildBackendMetadataError::from(err),
                    ));
                }
            };

            match metadata.modified() {
                Ok(modified_date) => {
                    if modified_date > cache_entry.timestamp {
                        tracing::trace!(
                            "found cached outputs but '{}' has been modified, invalidating cache.",
                            path.display()
                        );
                        return Ok(None);
                    }
                }
                Err(err) => {
                    tracing::trace!(
                        "found cached outputs but requested metadata for '{}' failed with: {}",
                        path.display(),
                        err
                    );
                    return Ok(None);
                }
            }
        }

        tracing::trace!("found up-to-date cached outputs.");
        Ok(Some(cache_entry))
    }

    /// Validates that outputs with the same name have unique variants.
    #[allow(clippy::result_large_err)]
    fn validate_unique_variants(
        outputs: &[pixi_build_types::procedures::conda_outputs::CondaOutput],
    ) -> Result<(), CommandDispatcherError<BuildBackendMetadataError>> {
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
                return Err(CommandDispatcherError::Failed(
                    BuildBackendMetadataError::DuplicateVariants {
                        package: package_name.as_normalized().to_string(),
                        duplicates: duplicate_variants.join(", "),
                    },
                ));
            }
        }

        Ok(())
    }

    /// Use the `conda/outputs` procedure to get the metadata for the source
    /// checkout.
    async fn call_conda_outputs(
        self,
        command_dispatcher: CommandDispatcher,
        build_source_checkout: SourceCheckout,
        project_model_hash: Option<ProjectModelHash>,
        backend: Backend,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<CachedCondaMetadata, CommandDispatcherError<BuildBackendMetadataError>> {
        let backend_identifier = backend.identifier().to_string();
        let params = CondaOutputsParams {
            channels: self.channels,
            host_platform: self.build_environment.host_platform,
            build_platform: self.build_environment.build_platform,
            variant_configuration: self.variant_configuration.clone().map(|variants| {
                variants
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
                    .collect()
            }),
            variant_files: self.variant_files.clone(),
            work_directory: command_dispatcher.cache_dirs().working_dirs().join(
                WorkDirKey {
                    source: SourceRecordOrCheckout::Checkout {
                        checkout: build_source_checkout.clone(),
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
        let timestamp = SystemTime::now();

        // If the backend supports unique variants, validate that outputs with the same name
        // have unique variants
        if backend.api_version().supports_unique_variants() {
            Self::validate_unique_variants(&outputs.outputs)?;
        }

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
            &build_source_checkout,
        );
        tracing::debug!(
            backend = %backend_identifier,
            source = %build_source_checkout.pinned,
            glob_count = input_globs.len(),
            "computing metadata input hash",
        );
        let (input_globs, input_files) = split_globs(input_globs);

        Ok(CachedCondaMetadata {
            id: CachedCondaMetadataId::random(),
            cache_version: 0,
            outputs: outputs.outputs,
            build_variants: self.variant_configuration.unwrap_or_default(),
            input_globs,
            input_files,
            build_source: build_source_checkout.pinned,
            project_model_hash,
            timestamp,
        })
    }

    /// Computes the cache key for this instance
    pub(crate) fn cache_shard(&self) -> BuildBackendMetadataCacheShard {
        BuildBackendMetadataCacheShard {
            channel_urls: self.channels.clone(),
            build_environment: self.build_environment.clone(),
            enabled_protocols: self.enabled_protocols.clone(),
            pinned_source: self.manifest_source.clone(),
        }
    }
}

/// Returns the input glob set extended with any variant file paths
/// relative to the source checkout root.
/// Paths are normalised to use forward slashes so that they are glob-compatible.
fn extend_input_globs_with_variant_files(
    mut input_globs: BTreeSet<String>,
    variant_files: &Option<Vec<PathBuf>>,
    source_checkout: &SourceCheckout,
) -> BTreeSet<String> {
    if let Some(variant_files) = variant_files {
        for variant_file in variant_files {
            let relative = match variant_file.strip_prefix(&source_checkout.path) {
                Ok(stripped) => stripped.to_path_buf(),
                Err(_) => diff_paths(variant_file, &source_checkout.path)
                    .unwrap_or_else(|| variant_file.clone()),
            };
            let glob = relative.to_string_lossy().replace("\\", "/");
            input_globs.insert(glob);
        }
    }
    input_globs
}

/// Splits globs into globs and explicit paths.
fn split_globs<G: Extend<String> + Default, F: Extend<PathBuf> + Default>(
    input_globs: BTreeSet<String>,
) -> (G, F) {
    input_globs.into_iter().partition_map(|glob| {
        if glob.contains("*") {
            Either::Left(glob)
        } else {
            Either::Right(PathBuf::from(glob))
        }
    })
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

    #[error(
        "the build backend returned outputs with duplicate variants for package '{package}': {duplicates}"
    )]
    DuplicateVariants { package: String, duplicates: String },

    #[error("could not compute hash of input files")]
    GlobHash(#[from] pixi_glob::GlobHashError),

    #[error("failed to determine input file modification times")]
    GlobSet(#[from] pixi_glob::GlobSetError),

    #[error(transparent)]
    Cache(#[from] build_backend_metadata::BuildBackendMetadataCacheError),
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

        let result = BuildBackendMetadataSpec::validate_unique_variants(&outputs);
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

        let result = BuildBackendMetadataSpec::validate_unique_variants(&outputs);
        assert!(
            result.is_err(),
            "Expected validation to fail for duplicate variants"
        );

        if let Err(CommandDispatcherError::Failed(BuildBackendMetadataError::DuplicateVariants {
            package,
            duplicates,
        })) = result
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

        let result = BuildBackendMetadataSpec::validate_unique_variants(&outputs);
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

        let result = BuildBackendMetadataSpec::validate_unique_variants(&outputs);
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

        let result = BuildBackendMetadataSpec::validate_unique_variants(&outputs);
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

        let result = BuildBackendMetadataSpec::validate_unique_variants(&outputs);
        assert!(
            result.is_err(),
            "Expected validation to fail for duplicate multi-key variants"
        );
    }
}
