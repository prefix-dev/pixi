mod cache;

use std::{
    ffi::OsStr,
    ops::Not,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use chrono::Utc;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_frontend::SetupRequest;
use pixi_build_types::{
    procedures::{
        conda_build::{CondaBuildParams, CondaOutputIdentifier},
        conda_metadata::CondaMetadataParams,
    },
    ChannelConfiguration, CondaPackageMetadata,
};
pub use pixi_glob::{GlobHashCache, GlobHashError};
use pixi_glob::{GlobHashKey, GlobModificationTime, GlobModificationTimeError};
use pixi_record::{InputHash, PinnedPathSpec, PinnedSourceSpec, SourceRecord};
use pixi_spec::SourceSpec;
use rattler_conda_types::{ChannelConfig, PackageRecord, Platform, RepoDataRecord};
use rattler_digest::Sha256;
use thiserror::Error;
use tracing::instrument;
use typed_path::{Utf8TypedPath, Utf8TypedPathBuf};
use url::Url;

use crate::build::cache::{
    BuildCache, BuildInput, CachedBuild, CachedCondaMetadata, SourceInfo, SourceMetadataCache,
    SourceMetadataInput,
};

/// The [`BuildContext`] is used to build packages from source.
#[derive(Clone)]
pub struct BuildContext {
    channel_config: ChannelConfig,
    glob_hash_cache: GlobHashCache,
    source_metadata_cache: SourceMetadataCache,
    build_cache: BuildCache,
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildError {
    #[error("failed to resolve source path {}", &.0)]
    ResolveSourcePath(Utf8TypedPathBuf, #[source] std::io::Error),

    #[error("error calculating sha for {}", &.0.display())]
    CalculateSha(PathBuf, #[source] std::io::Error),

    #[error(transparent)]
    BuildFrontendSetup(pixi_build_frontend::BuildFrontendError),

    #[error(transparent)]
    BackendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    FrontendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    InputHash(#[from] GlobHashError),

    #[error(transparent)]
    GlobModificationError(#[from] GlobModificationTimeError),

    #[error(transparent)]
    SourceMetadataError(#[from] cache::SourceMetadataError),

    #[error(transparent)]
    BuildCacheError(#[from] cache::BuildCacheError),
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
    pub fn new(cache_dir: PathBuf, channel_config: ChannelConfig) -> Self {
        Self {
            channel_config,
            glob_hash_cache: GlobHashCache::default(),
            source_metadata_cache: SourceMetadataCache::new(cache_dir.clone()),
            build_cache: BuildCache::new(cache_dir),
        }
    }

    /// Sets the input hash cache to use for caching input hashes.
    pub fn with_glob_hash_cache(self, glob_hash_cache: GlobHashCache) -> Self {
        Self {
            glob_hash_cache,
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
        let records = self
            .extract_records(&source, channels, target_platform)
            .await?;

        Ok(SourceMetadata { source, records })
    }

    /// Build a package from the given source specification.
    #[instrument(skip_all, fields(source = %source_spec.source))]
    pub async fn build_source_record(
        &self,
        source_spec: &SourceRecord,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<RepoDataRecord, BuildError> {
        let source_checkout = SourceCheckout {
            path: self.fetch_pinned_source(&source_spec.source).await?,
            pinned: source_spec.source.clone(),
        };

        let (cached_build, entry) = self
            .build_cache
            .entry(
                &source_checkout,
                &BuildInput {
                    channel_urls: channels.to_vec(),
                    target_platform: Platform::from_str(&source_spec.package_record.subdir)
                        .ok()
                        .unwrap_or(target_platform),
                    name: source_spec.package_record.name.as_normalized().to_string(),
                    version: source_spec.package_record.version.to_string(),
                    build: source_spec.package_record.build.clone(),
                },
            )
            .await?;

        if let Some(build) = cached_build {
            // Check to see if the cached build is up-to-date.
            if let Some(source_input) = build.source {
                let glob_time = GlobModificationTime::from_patterns(
                    &source_checkout.path,
                    source_input.globs.iter().map(String::as_str),
                )
                .map_err(BuildError::GlobModificationError)?;
                match glob_time {
                    GlobModificationTime::MatchesFound {
                        modified_at,
                        designated_file,
                    } => {
                        if build
                            .record
                            .package_record
                            .timestamp
                            .map(|t| t >= chrono::DateTime::<Utc>::from(modified_at))
                            .unwrap_or(false)
                        {
                            tracing::debug!("found an up-to-date cached build.");
                            return Ok(build.record);
                        } else {
                            tracing::debug!(
                                "found an stale cached build, {} is newer than {}",
                                designated_file.display(),
                                build.record.package_record.timestamp.unwrap_or_default()
                            );
                        }
                    }
                    GlobModificationTime::NoMatches => {
                        // No matches, so we should rebuild.
                        tracing::debug!(
                            "found a stale cached build, no files match the source glob"
                        );
                    }
                }
            } else {
                tracing::debug!("found a cached build");

                // If there is no source info in the cache we assume its still valid.
                return Ok(build.record);
            }
        }

        // Instantiate a protocol for the source directory.
        let protocol = pixi_build_frontend::BuildFrontend::default()
            .with_channel_config(self.channel_config.clone())
            .setup_protocol(SetupRequest {
                source_dir: source_checkout.path,
                build_tool_overrides: Default::default(),
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
                outputs: Some(vec![CondaOutputIdentifier {
                    name: Some(source_spec.package_record.name.as_normalized().to_string()),
                    version: Some(source_spec.package_record.version.version().to_string()),
                    build: Some(source_spec.package_record.build.clone()),
                    subdir: Some(source_spec.package_record.subdir.clone()),
                }]),
            })
            .await
            .map_err(|e| BuildError::BackendError(e.into()))?;

        let build_result = build_result
            .packages
            .into_iter()
            .exactly_one()
            .map_err(|e| {
                BuildError::FrontendError(
                    miette::miette!("expected the build backend to return a single built package but it returned {}", e.len())
                        .into(),
                )
            })?;

        // Add the sha256 to the package record.
        let sha = rattler_digest::compute_file_digest::<Sha256>(&build_result.output_file)
            .map_err(|e| BuildError::CalculateSha(build_result.output_file.clone(), e))?;

        // Update the package_record sha256 field and timestamp.
        let mut package_record = source_spec.package_record.clone();
        package_record.sha256 = Some(sha);
        package_record.timestamp.get_or_insert_with(Utc::now);

        // Construct a repodata record that represents the package
        let record = RepoDataRecord {
            package_record,
            url: Url::from_file_path(&build_result.output_file).map_err(|_| {
                BuildError::FrontendError(
                    miette::miette!(
                        "failed to convert returned path to URL: {}",
                        build_result.output_file.display()
                    )
                    .into(),
                )
            })?,
            channel: String::new(),
            file_name: build_result
                .output_file
                .file_name()
                .and_then(OsStr::to_str)
                .map(ToString::to_string)
                .unwrap_or_default(),
        };

        // Store the build in the cache
        let updated_record = entry
            .insert(CachedBuild {
                source: source_checkout
                    .pinned
                    .is_immutable()
                    .not()
                    .then_some(SourceInfo {
                        globs: build_result.input_globs,
                    }),
                record: record.clone(),
            })
            .await?;

        Ok(updated_record)
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
    #[instrument(skip_all, fields(source = %source.pinned, platform = %target_platform))]
    async fn extract_records(
        &self,
        source: &SourceCheckout,
        channels: &[Url],
        target_platform: Platform,
    ) -> Result<Vec<SourceRecord>, BuildError> {
        let (cached_metadata, cache_entry) = self
            .source_metadata_cache
            .entry(
                source,
                &SourceMetadataInput {
                    channel_urls: channels.to_vec(),
                    target_platform,
                },
            )
            .await?;
        if let Some(metadata) = cached_metadata {
            // Check if the input hash is still valid.
            if let Some(input_globs) = &metadata.input_hash {
                let new_hash = self
                    .glob_hash_cache
                    .compute_hash(GlobHashKey {
                        root: source.path.clone(),
                        globs: input_globs.globs.clone(),
                    })
                    .await?;
                if new_hash.hash == input_globs.hash {
                    tracing::debug!("found up-to-date cached metadata.");
                    return Ok(source_metadata_to_records(
                        source,
                        metadata.packages,
                        metadata.input_hash,
                    ));
                } else {
                    tracing::debug!("found stale cached metadata.");
                }
            } else {
                tracing::debug!("found cached metadata.");

                // No input hash so just assume it is still valid.
                return Ok(source_metadata_to_records(
                    source,
                    metadata.packages,
                    metadata.input_hash,
                ));
            }
        }

        // Instantiate a protocol for the source directory.
        let protocol = pixi_build_frontend::BuildFrontend::default()
            .with_channel_config(self.channel_config.clone())
            .setup_protocol(SetupRequest {
                source_dir: source.path.clone(),
                build_tool_overrides: Default::default(),
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
            let input_globs = metadata.input_globs.clone().unwrap_or(protocol.manifests());
            let input_hash = self
                .glob_hash_cache
                .compute_hash(GlobHashKey {
                    root: source.path.clone(),
                    globs: input_globs.clone(),
                })
                .await?;
            Some(InputHash {
                hash: input_hash.hash,
                globs: input_globs,
            })
        };

        // Store in the cache
        cache_entry
            .insert(CachedCondaMetadata {
                packages: metadata.packages.clone(),
                input_hash: input_hash.clone(),
            })
            .await?;

        Ok(source_metadata_to_records(
            source,
            metadata.packages,
            input_hash,
        ))
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
    packages
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
