use std::path::PathBuf;

use miette::Diagnostic;
use pixi_build_frontend::{BackendOverrides, SetupRequest};
use pixi_build_types::procedures::conda_metadata::{ChannelConfiguration, CondaMetadataParams};
use pixi_spec::{PathSourceSpec, SourceSpec};
use rattler_conda_types::{
    package::{ArchiveIdentifier, ArchiveType},
    ChannelConfig, PackageRecord, Platform, RepoDataRecord,
};
use thiserror::Error;
use url::Url;

/// The [`BuildContext`] is used to build packages from source.
#[derive(Debug, Clone)]
pub struct BuildContext {
    channel_config: ChannelConfig,
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildError {
    #[error("failed to resolve source path {}", &.0.path)]
    ResolveSourcePath(PathSourceSpec, #[source] std::io::Error),

    #[error("failed to construct build frontend")]
    BuildFrontendSetup(#[source] pixi_build_frontend::BuildFrontendError),

    #[error("failed to retrieve package metadata")]
    ExtractMetadata(#[source] pixi_build_frontend::BuildFrontendError),
}

/// Location of the source code for a package. This will be used as the input
/// for the build process. Archives are unpacked, git clones are checked out,
/// etc.
#[derive(Default)]
pub struct SourceCheckout {
    pub path: PathBuf,
    // TODO(baszalmstra): Add source pinning information, e.g. the commit hash for git, the sha
    //  hash for a url, etc.
}

impl BuildContext {
    /// Extracts the metadata for a package from the given source specification.
    pub async fn extract_source_metadata(
        &self,
        source_spec: &SourceSpec,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<Vec<RepoDataRecord>, BuildError> {
        let source = self.fetch_source(source_spec).await?;

        // TODO: Add caching of this information based on the source.

        self.extract_metadata(&source, channels, target_platform)
            .await
    }

    /// Acquires the source from the given source specification. A source
    /// specification can still not point to a specific pinned source. E.g. a
    /// git spec that points to a branch or a tag. This function will fetch the
    /// source and return a [`SourceCheckout`] that points to the actual source.
    /// This also pins the source spec to a specific checkout (e.g. git commit
    /// hash).
    ///
    /// TODO(baszalmstra): Ideally we would cache the result of this on disk
    /// somewhere.
    pub async fn fetch_source(
        &self,
        source_spec: &SourceSpec,
    ) -> Result<SourceCheckout, BuildError> {
        match source_spec {
            SourceSpec::Url(_) => unimplemented!("fetching URL sources is not yet implemented"),
            SourceSpec::Git(_) => unimplemented!("fetching Git sources is not yet implemented"),
            SourceSpec::Path(path) => {
                let source_path = path
                    .resolve(&self.channel_config.root_dir)
                    .map_err(|err| BuildError::ResolveSourcePath(path.clone(), err))?;
                Ok(SourceCheckout { path: source_path })
            }
        }
    }

    /// Extracts the metadata from a package whose source is located at the
    /// given path.
    async fn extract_metadata(
        &self,
        source: &SourceCheckout,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<Vec<RepoDataRecord>, BuildError> {
        // Instantiate a protocol for the source directory.
        let protocol = pixi_build_frontend::BuildFrontend::default()
            .with_channel_config(self.channel_config.clone())
            .setup_protocol(SetupRequest {
                source_dir: source.path.clone(),
                build_tool_overrides: BackendOverrides {
                    spec: None,
                    path: Some("pixi-build-python".into()),
                },
            })
            .await
            .map_err(|e| BuildError::BuildFrontendSetup(e))?;

        // Extract the conda metadata for the package.
        let metadata = protocol
            .get_conda_metadata(&CondaMetadataParams {
                target_platform: Some(target_platform),
                channel_base_urls: Some(channels.to_owned()),
                channel_configuration: ChannelConfiguration {
                    base_url: self.channel_config.channel_alias.clone(),
                },
            })
            .await
            .map_err(|e| BuildError::ExtractMetadata(e))?;

        // Convert the metadata to repodata
        Ok(metadata
            .packages
            .into_iter()
            .map(|p| {
                let file_name = ArchiveIdentifier {
                    name: p.name.as_normalized().to_string(),
                    version: p.version.to_string(),
                    build_string: p.build.clone(),
                    archive_type: ArchiveType::Conda,
                }
                .to_file_name();

                // TODO(baszalmstra): Figure out something much better than this.
                let archive_path = source.path.join(&file_name);
                let url = Url::from_directory_path(&archive_path).expect("invalid source path");

                RepoDataRecord {
                    // TODO(baszalmstra): Figure out what to do with this value
                    channel: "".to_string(),

                    file_name,
                    url,
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

                        // TODO(baszalmstra): Add support for these.
                        purls: None,

                        // These are not important at this point.
                        run_exports: None,
                    },
                }
            })
            .collect())
    }
}
