use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use miette::Diagnostic;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::{
    Backend,
    types::{
        ChannelConfiguration, CondaPackageMetadata, PlatformAndVirtualPackages,
        SourcePackageSpecV1, procedures::conda_metadata::CondaMetadataParams,
    },
};
use pixi_build_types::procedures::conda_outputs::{
    CondaOutputMetadata, CondaOutputsParams, CondaOutputsResult,
};
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

use crate::source_metadata::source_metadata_cache::{CacheEntry, CachedCondaMetadata};

#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
pub struct BuildSourceRecordSpec {
    /// The name of the package to build.
    pub package: PackageName,

    /// The request for source metadata from which to construct the record.
    pub source_metadata: SourceMetadataSpec,
}

/// The result of building a particular source record.
pub struct BuildSourceRecordResult {
    pub records: Vec<SourceRecord>,
}

impl BuildSourceRecordSpec {
    pub(crate) async fn request(
        self,
        command_dispatcher: CommandDispatcher,
    ) -> Result<BuildSourceRecordResult, CommandDispatcherError<SourceMetadataError>> {

    }

    /// Use the `conda/outputs` procedure to get the metadata for the source
    /// checkout.
    async fn call_conda_outputs(
        self,
        command_dispatcher: CommandDispatcher,
        entry: CacheEntry,
        manifest_path: PathBuf,
        backend: Backend,
    ) -> Result<SourceMetadata, CommandDispatcherError<SourceMetadataError>> {
        let params = CondaOutputsParams {
            host_platform: self.build_environment.host_platform,
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
        let outputs = backend
            .conda_outputs(params)
            .await
            .map_err(SourceMetadataError::Communication)
            .map_err(CommandDispatcherError::Failed)?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = Self::compute_input_hash(
            command_dispatcher,
            &self.source,
            manifest_path,
            outputs.input_globs.clone(),
        )
        .await?;

        // Store the metadata in the cache for later retrieval
        entry
            .insert(CachedCondaMetadata {
                input_hash: input_hash.clone(),
                packages: Vec::new(),
                outputs: outputs.outputs.clone(),
            })
            .await
            .map_err(SourceMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(SourceMetadata {
            metadata: Metadata::Outputs(CondaOutputsOutput {
                outputs: outputs.outputs,
                input_hash,
            }),
            source: self.source,
        })
    }

    /// Use the `conda/getMetadata` procedure to get the metadata for the source
    async fn call_conda_get_metadata(
        self,
        command_dispatcher: CommandDispatcher,
        entry: CacheEntry,
        manifest_path: PathBuf,
        backend: Backend,
    ) -> Result<SourceMetadata, CommandDispatcherError<SourceMetadataError>> {
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
            manifest_path,
            metadata.input_globs.clone(),
        )
        .await?;

        // Store the metadata in the cache for later retrieval
        entry
            .insert(CachedCondaMetadata {
                input_hash: input_hash.clone(),
                packages: metadata.packages.clone(),
                outputs: Vec::new(),
            })
            .await
            .map_err(SourceMetadataError::Cache)
            .map_err(CommandDispatcherError::Failed)?;

        Ok(SourceMetadata {
            metadata: Metadata::GetMetadata(CondaGetMetadataOutput {
                packages: source_metadata_to_records(&self.source, metadata.packages, input_hash),
            }),
            source: self.source,
        })
    }

    /// Computes the input hash for metadata returned by the backend.
    async fn compute_input_hash(
        command_queue: CommandDispatcher,
        source: &SourceCheckout,
        manifest_path: PathBuf,
        input_globs: Option<BTreeSet<String>>,
    ) -> Result<Option<InputHash>, CommandDispatcherError<SourceMetadataError>> {
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
