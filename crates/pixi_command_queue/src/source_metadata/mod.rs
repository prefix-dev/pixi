use miette::Diagnostic;
use pixi_build_frontend::{
    DiscoveredBackend, EnabledProtocols,
    types::{
        ChannelConfiguration, CondaPackageMetadata, PlatformAndVirtualPackages,
        SourcePackageSpecV1,
        procedures::conda_metadata::{CondaMetadataParams, CondaMetadataResult},
    },
};
use pixi_glob::GlobHashKey;
use pixi_record::{InputHash, SourceRecord};
use pixi_spec::SourceSpec;
use rattler_conda_types::{ChannelConfig, ChannelUrl, PackageRecord};
use thiserror::Error;

use crate::{
    BuildEnvironment, CommandQueue, CommandQueueError, CommandQueueErrorResultExt,
    InstantiateBackendError, InstantiateBackendSpec, SourceCheckout, SourceCheckoutError,
    build::WorkDirKey,
};

/// Represents a request for source metadata.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct SourceMetadataSpec {
    /// The source specification
    pub source_spec: SourceSpec,

    /// The channel configuration to use when resolving metadata
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    pub channels: Vec<ChannelUrl>,

    /// Information about the build environment.
    pub build_environment: BuildEnvironment,

    /// The protocols that are enabled for this source
    pub enabled_protocols: EnabledProtocols,
}

/// The metadata of a source checkout.
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// The source checkout that the manifest was extracted from.
    pub source: SourceCheckout,

    /// All the records that can be extracted from the source.
    pub records: Vec<SourceRecord>,
}

impl SourceMetadataSpec {
    pub(crate) async fn request(
        self,
        command_queue: CommandQueue,
    ) -> Result<SourceMetadata, CommandQueueError<SourceMetadataError>> {
        // Get the pinned source for this source spec.
        let source = command_queue
            .pin_and_checkout(self.source_spec.clone())
            .await
            .map_err_with(SourceMetadataError::SourceCheckout)?;

        // Discover information about the build backend from the source code.
        let discovered_backend = DiscoveredBackend::discover(
            &source.path,
            &self.channel_config,
            &self.enabled_protocols,
        )
        .map_err(SourceMetadataError::Discovery)?;

        // Instantiate the backend with the discovered backend information.
        let backend = command_queue
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend.backend_spec,
                init_params: discovered_backend.init_params,
                channel_config: self.channel_config.clone(),
                build_environment: BuildEnvironment {
                    host_platform: self.build_environment.build_platform.clone(),
                    host_virtual_packages: self.build_environment.build_virtual_packages.clone(),
                    build_platform: self.build_environment.build_platform.clone(),
                    build_virtual_packages: self.build_environment.build_virtual_packages.clone(),
                },
                enabled_protocols: self.enabled_protocols
            })
            .await
            .map_err_with(SourceMetadataError::Initialize)?;

        // Query the backend for metadata.
        let metadata = backend
            .conda_get_metadata(&CondaMetadataParams {
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
                variant_configuration: None,
                work_directory: command_queue.cache_dirs().working_dirs().join(
                    WorkDirKey {
                        source: source.clone(),
                        host_platform: self.build_environment.host_platform,
                        build_backend: backend.identifier().to_string(),
                    }
                    .key(),
                ),
            })
            .await
            .map_err(SourceMetadataError::Communication)?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = Self::compute_input_hash(command_queue, &source, &metadata).await?;

        Ok(SourceMetadata {
            records: source_metadata_to_records(&source, metadata.packages, input_hash),
            source,
        })
    }

    /// Computes the input hash for metadata returned by the backend.
    async fn compute_input_hash(
        command_queue: CommandQueue,
        source: &SourceCheckout,
        metadata: &CondaMetadataResult,
    ) -> Result<Option<InputHash>, CommandQueueError<SourceMetadataError>> {
        let input_hash = if source.pinned.is_immutable() {
            None
        } else {
            let input_globs = metadata
                .input_globs
                .clone()
                .into_iter()
                .flat_map(|glob| glob.into_iter())
                .collect::<Vec<_>>();

            let input_hash = command_queue
                .glob_hash_cache()
                .compute_hash(GlobHashKey {
                    root: source.path.clone(),
                    globs: input_globs.clone(),
                })
                .await
                .map_err(SourceMetadataError::GlobHash)?;

            Some(InputHash {
                hash: input_hash.hash,
                globs: input_globs,
            })
        };
        Ok(input_hash)
    }
}

fn source_metadata_to_records(
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
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(#[from] pixi_build_frontend::DiscoveryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Initialize(#[from] InstantiateBackendError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Communication(#[from] pixi_build_frontend::json_rpc::CommunicationError),

    #[error("could not compute hash of input files")]
    GlobHash(#[from] pixi_glob::GlobHashError),
}
