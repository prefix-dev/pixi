use std::{
    collections::{BTreeMap, BTreeSet},
    hash::Hash,
};

use miette::Diagnostic;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::types::{
    ChannelConfiguration, CondaPackageMetadata, PlatformAndVirtualPackages, SourcePackageSpecV1,
    procedures::conda_metadata::CondaMetadataParams,
};
use pixi_build_type_conversions::compute_project_model_hash;
use pixi_glob::GlobHashKey;
use pixi_record::{InputHash, SourceRecord};
use rattler_conda_types::{ChannelConfig, ChannelUrl, PackageRecord};
use thiserror::Error;

use crate::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    InstantiateBackendError, InstantiateBackendSpec, SourceCheckout, build::WorkDirKey,
};

mod source_metadata_cache;

use source_metadata_cache::SourceMetadataKey;
pub use source_metadata_cache::{SourceMetadataCache, SourceMetadataCacheError};

use crate::source_metadata::source_metadata_cache::CachedCondaMetadata;

/// Represents a request for source metadata.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct SourceMetadataSpec {
    /// The source specification
    pub source: SourceCheckout,

    /// The channel configuration to use when resolving metadata
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMetadata {
    /// The source checkout that the manifest was extracted from.
    pub source: SourceCheckout,

    /// All the records that can be extracted from the source.
    pub records: Vec<SourceRecord>,
}

impl SourceMetadataSpec {
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<SourceMetadata, CommandDispatcherError<SourceMetadataError>> {
        tracing::debug!(
            "Requesting source metadata for source spec: {}",
            self.source.pinned
        );

        // Discover information about the build backend from the source code.
        let discovered_backend = DiscoveredBackend::discover(
            &self.source.path,
            &self.channel_config,
            &self.enabled_protocols,
        )
        .map_err(SourceMetadataError::Discovery)
        .map_err(CommandDispatcherError::Failed)?;

        // Check the source metadata cache, short circuit if we have it.
        let cache_key = self.cache_key();
        let (metadata, entry) = command_dispatcher
            .source_metadata_cache()
            .entry(&self.source.pinned, &cache_key)
            .await
            .map_err(SourceMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        // Calculate the hash of the project model
        let project_model_hash = discovered_backend
            .init_params
            .project_model
            .as_ref()
            .map(compute_project_model_hash);

        if let Some(metadata) = metadata {
            tracing::debug!(
                "Found source metadata in cache for source spec: {}",
                self.source.pinned
            );

            // Check if the input hash is still valid.
            if let Some(input_globs) = &metadata.input_hash {
                let new_hash = command_dispatcher
                    .glob_hash_cache()
                    .compute_hash(GlobHashKey::new(
                        self.source.path.clone(),
                        input_globs.globs.clone(),
                        project_model_hash.clone(),
                    ))
                    .await
                    .map_err(SourceMetadataError::GlobHash)
                    .map_err(CommandDispatcherError::Failed)?;
                if new_hash.hash == input_globs.hash {
                    tracing::debug!("found up-to-date cached metadata.");
                    return Ok(SourceMetadata {
                        records: source_metadata_to_records(
                            &self.source,
                            metadata.packages,
                            metadata.input_hash,
                        ),
                        source: self.source,
                    });
                } else {
                    tracing::debug!("found stale cached metadata.");
                }
            } else {
                tracing::debug!("found cached metadata.");
                // No input hash so just assume it is still valid.
                return Ok(SourceMetadata {
                    records: source_metadata_to_records(
                        &self.source,
                        metadata.packages,
                        metadata.input_hash,
                    ),
                    source: self.source,
                });
            }
        }

        // Instantiate the backend with the discovered information.
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend.backend_spec,
                init_params: discovered_backend.init_params,
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols,
            })
            .await
            .map_err_with(SourceMetadataError::Initialize)?;

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
                    source: Box::new(self.source.clone()).into(),
                    host_platform: self.build_environment.host_platform,
                    build_backend: backend.identifier().to_string(),
                }
                .key(),
            ),
        };
        let metadata = backend
            .conda_get_metadata(params)
            .await
            .map_err(SourceMetadataError::Communication)
            .map_err(CommandDispatcherError::Failed)?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = Self::compute_input_hash(
            command_dispatcher,
            &self.source,
            project_model_hash,
            metadata.input_globs,
        )
        .await?;

        // Store the metadata in the cache for later retrieval
        entry
            .insert(CachedCondaMetadata {
                input_hash: input_hash.clone(),
                packages: metadata.packages.clone(),
            })
            .await
            .map_err(SourceMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(SourceMetadata {
            records: source_metadata_to_records(&self.source, metadata.packages, input_hash),
            source: self.source,
        })
    }

    /// Computes the input hash for metadata returned by the backend.
    async fn compute_input_hash(
        command_queue: CommandDispatcher,
        source: &SourceCheckout,
        project_model_hash: Option<Vec<u8>>,
        input_globs: Option<BTreeSet<String>>,
    ) -> Result<Option<InputHash>, CommandDispatcherError<SourceMetadataError>> {
        let input_hash = if source.pinned.is_immutable() {
            None
        } else {
            // Compute the input hash based on the project model and the input globs.
            let input_globs = input_globs.unwrap_or_default();
            let input_hash = command_queue
                .glob_hash_cache()
                .compute_hash(GlobHashKey::new(
                    &source.path,
                    input_globs.clone(),
                    project_model_hash,
                ))
                .await
                .map_err(SourceMetadataError::GlobHash)
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
        }
    }
}

pub(crate) fn source_metadata_to_records(
    source: &SourceCheckout,
    packages: Vec<CondaPackageMetadata>,
    input_hash: Option<InputHash>,
) -> Vec<SourceRecord> {
    // Convert the metadata to repodata
    let packages = packages
        .into_iter()
        .map(|p| {
            SourceRecord {
                input_hash: input_hash.clone(),
                source: source.pinned.clone(),
                sources: p
                    .sources
                    .into_iter()
                    .map(|(name, source)| (name, from_pixi_source_spec_v1(source)))
                    .collect(),
                package_record: PackageRecord {
                    // We cannot now these values from the metadata because no actual package
                    // was built yet.
                    size: None,
                    sha256: None,
                    md5: None,

                    // TODO(baszalmstra): Decide if it makes sense to include the current
                    // timestamp here.
                    timestamp: None,

                    // These values are derived from the build backend values.
                    platform: p.subdir.only_platform().map(ToString::to_string),
                    arch: p.subdir.arch().as_ref().map(ToString::to_string),

                    // These values are passed by the build backend
                    name: p.name,
                    build: p.build,
                    version: p.version,
                    build_number: p.build_number,
                    license: p.license,
                    subdir: p.subdir.to_string(),
                    license_family: p.license_family,
                    noarch: p.noarch,
                    constrains: p.constraints.into_iter().map(|c| c.to_string()).collect(),
                    depends: p.depends.into_iter().map(|c| c.to_string()).collect(),

                    // These are deprecated and no longer used.
                    features: None,
                    track_features: vec![],
                    legacy_bz2_md5: None,
                    legacy_bz2_size: None,
                    python_site_packages_path: None,

                    // TODO(baszalmstra): Add support for these.
                    purls: None,

                    // These are not important at this point.
                    run_exports: None,
                    extra_depends: Default::default(),
                },
            }
        })
        .collect();
    packages
}

pub fn from_pixi_source_spec_v1(source: SourcePackageSpecV1) -> pixi_spec::SourceSpec {
    match source {
        SourcePackageSpecV1::Url(url) => pixi_spec::SourceSpec::Url(pixi_spec::UrlSourceSpec {
            url: url.url,
            md5: url.md5,
            sha256: url.sha256,
        }),
        SourcePackageSpecV1::Git(git) => pixi_spec::SourceSpec::Git(pixi_spec::GitSpec {
            git: git.git,
            rev: git.rev.map(|r| match r {
                pixi_build_frontend::types::GitReferenceV1::Branch(b) => {
                    pixi_spec::GitReference::Branch(b)
                }
                pixi_build_frontend::types::GitReferenceV1::Tag(t) => {
                    pixi_spec::GitReference::Tag(t)
                }
                pixi_build_frontend::types::GitReferenceV1::Rev(rev) => {
                    pixi_spec::GitReference::Rev(rev)
                }
                pixi_build_frontend::types::GitReferenceV1::DefaultBranch => {
                    pixi_spec::GitReference::DefaultBranch
                }
            }),
            subdirectory: git.subdirectory,
        }),
        SourcePackageSpecV1::Path(path) => pixi_spec::SourceSpec::Path(pixi_spec::PathSourceSpec {
            path: path.path.into(),
        }),
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum SourceMetadataError {
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
    Cache(#[from] SourceMetadataCacheError),
}
