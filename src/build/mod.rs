use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use miette::Diagnostic;
use pixi_build_frontend::{BackendOverrides, SetupRequest};
use pixi_build_types::{
    procedures::{
        conda_build::{CondaBuildParams, CondaOutputIdentifier},
        conda_metadata::CondaMetadataParams,
    },
    ChannelConfiguration,
};
use pixi_record::{InputHash, InputHashError, PinnedPathSpec, PinnedSourceSpec, SourceRecord};
use pixi_spec::SourceSpec;
use rattler_conda_types::{ChannelConfig, PackageRecord, Platform, RepoDataRecord};
use thiserror::Error;
use typed_path::{Utf8TypedPath, Utf8TypedPathBuf};
use url::Url;

mod input_hash_cache;
pub use input_hash_cache::{InputHashCache, InputHashKey};

/// The [`BuildContext`] is used to build packages from source.
#[derive(Debug, Clone)]
pub struct BuildContext {
    channel_config: ChannelConfig,
    _input_hash_cache: InputHashCache,
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildError {
    #[error("failed to resolve source path {}", &.0)]
    ResolveSourcePath(Utf8TypedPathBuf, #[source] std::io::Error),

    #[error(transparent)]
    BuildFrontendSetup(pixi_build_frontend::BuildFrontendError),

    #[error(transparent)]
    BackendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    FrontendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    InputHash(#[from] InputHashError),
}

/// Location of the source code for a package. This will be used as the input
/// for the build process. Archives are unpacked, git clones are checked out,
/// etc.
#[derive(Debug)]
pub struct SourceCheckout {
    /// The path to where the source is located locally on disk.
    pub path: PathBuf,

    /// The exact source specification
    pub pinned: PinnedSourceSpec,
}

/// The metadata of a source checkout.
#[derive(Debug)]
pub struct SourceMetadata {
    /// The source checkout that the manifest was extracted from.
    pub source: SourceCheckout,

    /// All the records that can be extracted from the source.
    pub records: Vec<SourceRecord>,
}

impl BuildContext {
    pub fn new(channel_config: ChannelConfig) -> Self {
        Self {
            channel_config,
            _input_hash_cache: InputHashCache::default(),
        }
    }

    /// Sets the input hash cache to use for caching input hashes.
    pub fn with_input_hash_cache(self, input_hash_cache: InputHashCache) -> Self {
        Self {
            _input_hash_cache: input_hash_cache,
            ..self
        }
    }

    /// Extracts the metadata for a package from the given source specification.
    pub async fn extract_source_metadata(
        &self,
        source_spec: &SourceSpec,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<SourceMetadata, BuildError> {
        let source = self.fetch_source(source_spec).await?;

        // TODO: Add caching of this information based on the source.

        let records = self
            .extract_records(&source, channels, target_platform)
            .await?;

        Ok(SourceMetadata { source, records })
    }

    /// Build a package from the given source specification.
    pub async fn build_source_record(
        &self,
        source_spec: &SourceRecord,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<RepoDataRecord, BuildError> {
        let source = self.fetch_pinned_source(&source_spec.source).await?;

        // TODO: Add caching of this information based on the source.

        // Instantiate a protocol for the source directory.
        let protocol = pixi_build_frontend::BuildFrontend::default()
            .with_channel_config(self.channel_config.clone())
            .setup_protocol(SetupRequest {
                source_dir: source.clone(),
                build_tool_overrides: BackendOverrides {
                    spec: None,
                    path: Some("pixi-build-python".into()),
                },
            })
            .await
            .map_err(BuildError::BuildFrontendSetup)?;

        // Extract the conda metadata for the package.
        let build_result = protocol
            .conda_build(&CondaBuildParams {
                target_platform: Some(target_platform),
                channel_base_urls: Some(channels.to_owned()),
                channel_configuration: ChannelConfiguration {
                    base_url: self.channel_config.channel_alias.clone(),
                },
                output: CondaOutputIdentifier {
                    name: Some(source_spec.package_record.name.as_normalized().to_string()),
                    version: Some(source_spec.package_record.version.version().to_string()),
                    build: Some(source_spec.package_record.build.clone()),
                    subdir: Some(source_spec.package_record.subdir.clone()),
                },
            })
            .await
            .map_err(|e| BuildError::BackendError(e.into()))?;

        // Construct a repodata record that represents the package
        Ok(RepoDataRecord {
            package_record: source_spec.package_record.clone(),
            url: Url::from_file_path(&build_result.path).map_err(|_| {
                BuildError::FrontendError(
                    miette::miette!(
                        "failed to convert returned path to URL: {}",
                        build_result.path.display()
                    )
                    .into(),
                )
            })?,
            channel: String::new(),
            file_name: build_result
                .path
                .file_name()
                .and_then(OsStr::to_str)
                .map(ToString::to_string)
                .unwrap_or_default(),
        })
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
                let source_path = self
                    .resolve_path(path.path.to_path())
                    .map_err(|err| BuildError::ResolveSourcePath(path.path.clone(), err))?;
                Ok(SourceCheckout {
                    path: source_path,
                    pinned: PinnedPathSpec {
                        path: path.path.clone(),
                    }
                    .into(),
                })
            }
        }
    }

    /// Acquires the source from the given source specification.
    ///
    /// TODO(baszalmstra): Ideally we would cache the result of this on disk
    /// somewhere.
    pub async fn fetch_pinned_source(
        &self,
        source_spec: &PinnedSourceSpec,
    ) -> Result<PathBuf, BuildError> {
        match source_spec {
            PinnedSourceSpec::Url(_) => {
                unimplemented!("fetching URL sources is not yet implemented")
            }
            PinnedSourceSpec::Git(_) => {
                unimplemented!("fetching Git sources is not yet implemented")
            }
            PinnedSourceSpec::Path(path) => self
                .resolve_path(path.path.to_path())
                .map_err(|err| BuildError::ResolveSourcePath(path.path.clone(), err)),
        }
    }

    /// Resolves the source path to a full path.
    ///
    /// This function does not check if the path exists and also does not follow
    /// symlinks.
    fn resolve_path(&self, path_spec: Utf8TypedPath) -> Result<PathBuf, std::io::Error> {
        if path_spec.is_absolute() {
            Ok(Path::new(path_spec.as_str()).to_path_buf())
        } else if let Ok(user_path) = path_spec.strip_prefix("~/") {
            let home_dir = dirs::home_dir().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "could not determine home directory",
                )
            })?;
            debug_assert!(home_dir.is_absolute());
            normalize_absolute_path(&home_dir.join(Path::new(user_path.as_str())))
        } else {
            let root_dir = self.channel_config.root_dir.as_path();
            let native_path = Path::new(path_spec.as_str());
            debug_assert!(root_dir.is_absolute());
            normalize_absolute_path(&root_dir.join(native_path))
        }
    }

    /// Extracts the metadata from a package whose source is located at the
    /// given path.
    async fn extract_records(
        &self,
        source: &SourceCheckout,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<Vec<SourceRecord>, BuildError> {
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
            .map_err(BuildError::BuildFrontendSetup)?;

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
            .map_err(|e| BuildError::BackendError(e.into()))?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = if source.pinned.is_immutable() {
            None
        } else {
            let input_globs = metadata.input_globs.unwrap_or(protocol.manifests());
            let input_hash = InputHash::from_globs(&source.path, input_globs)?;
            Some(input_hash)
        };

        // Convert the metadata to repodata
        let packages = metadata
            .packages
            .into_iter()
            .map(|p| {
                SourceRecord {
                    input_hash: input_hash.clone(),
                    source: source.pinned.clone(),
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
            .collect();

        Ok(packages)
    }
}

/// Normalize a path, removing things like `.` and `..`.
///
/// Source: <https://github.com/rust-lang/cargo/blob/b48c41aedbd69ee3990d62a0e2006edbb506a480/crates/cargo-util/src/paths.rs#L76C1-L109C2>
fn normalize_absolute_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    let mut components = path.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().copied() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !ret.pop() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "cannot normalize a relative path beyond the base directory: {}",
                            path.display()
                        ),
                    ));
                }
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    Ok(ret)
}
