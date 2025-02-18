mod cache;
mod reporters;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use itertools::Itertools;
use miette::Diagnostic;
use miette::IntoDiagnostic;
use pixi_build_frontend::{BackendOverride, Protocol, SetupRequest, ToolContext};
use pixi_build_types::{
    procedures::{
        conda_build::{CondaBuildParams, CondaOutputIdentifier},
        conda_metadata::CondaMetadataParams,
    },
    ChannelConfiguration, CondaPackageMetadata, PlatformAndVirtualPackages,
};
use pixi_config::get_cache_dir;
use pixi_consts::consts::CACHED_GIT_DIR;
use pixi_git::{git::GitReference, resolver::GitResolver, source::Fetch, GitUrl, Reporter};
pub use pixi_glob::{GlobHashCache, GlobHashError};
use pixi_glob::{GlobHashKey, GlobModificationTime, GlobModificationTimeError};
use pixi_manifest::Targets;
use pixi_record::{
    InputHash, PinnedGitCheckout, PinnedGitSpec, PinnedPathSpec, PinnedSourceSpec, SourceRecord,
};
use pixi_spec::{GitSpec, SourceSpec};
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, GenericVirtualPackage, PackageRecord, Platform, RepoDataRecord,
};
use rattler_digest::Sha256;
use reporters::SourceReporter;
pub use reporters::{BuildMetadataReporter, BuildReporter, SourceCheckoutReporter};
use std::sync::LazyLock;
use std::{
    collections::HashMap,
    ffi::OsStr,
    hash::{Hash, Hasher},
    ops::Not,
    path::{Component, Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use thiserror::Error;
use tracing::instrument;
use typed_path::{Utf8TypedPath, Utf8TypedPathBuf};
use url::Url;
use uv_configuration::RAYON_INITIALIZE;
use xxhash_rust::xxh3::Xxh3;

use crate::build::cache::{
    BuildCache, BuildInput, CachedBuild, CachedCondaMetadata, SourceInfo, SourceMetadataCache,
    SourceMetadataInput,
};
use crate::Workspace;

/// A list of globs that should be ignored when calculating any input hash.
/// These are typically used for build artifacts that should not be included in
/// the input hash.
const DEFAULT_BUILD_IGNORE_GLOBS: &[&str] = &["!.pixi/**"];

/// The [`BuildContext`] is used to build packages from source.
#[derive(Clone)]
pub struct BuildContext {
    channel_config: ChannelConfig,
    glob_hash_cache: GlobHashCache,
    source_metadata_cache: SourceMetadataCache,
    build_cache: BuildCache,
    cache_dir: PathBuf,
    work_dir: PathBuf,
    tool_context: Arc<ToolContext>,
    variant_config: Targets<Option<HashMap<String, Vec<String>>>>,

    /// The resolved Git references.
    git: GitResolver,
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildError {
    #[error("failed to resolve path source {}", &.0)]
    ResolvePathSource(Utf8TypedPathBuf, #[source] std::io::Error),

    #[error("failed to resolve git source {}", &.0)]
    ResolveGitSource(Url, #[diagnostic_source] miette::Report),

    #[error("error calculating sha for {}", &.0.display())]
    CalculateSha(PathBuf, #[source] std::io::Error),

    #[error("failed to initialize a build backend for {}", &.0.pinned)]
    BuildFrontendSetup(
        Box<SourceCheckout>,
        #[diagnostic_source] pixi_build_frontend::BuildFrontendError,
    ),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    FrontendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    InputHash(#[from] GlobHashError),

    #[error(transparent)]
    GlobModificationError(#[from] GlobModificationTimeError),

    #[error(transparent)]
    SourceMetadataError(#[from] cache::SourceMetadataError),

    #[error(transparent)]
    BuildCacheError(#[from] cache::BuildCacheError),

    #[error(transparent)]
    BuildFolderNotWritable(#[from] std::io::Error),

    #[error(transparent)]
    FetchError(Box<dyn Diagnostic + Send + Sync + 'static>),
}

/// Location of the source code for a package. This will be used as the input
/// for the build process. Archives are unpacked, git clones are checked out,
/// etc.
#[derive(Debug, Clone)]
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
    pub fn new(
        cache_dir: PathBuf,
        dot_pixi_dir: PathBuf,
        channel_config: ChannelConfig,
        variant_config: Targets<Option<HashMap<String, Vec<String>>>>,
        tool_context: Arc<ToolContext>,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            channel_config,
            glob_hash_cache: GlobHashCache::default(),
            source_metadata_cache: SourceMetadataCache::new(cache_dir.clone()),
            build_cache: BuildCache::new(cache_dir.clone()),
            cache_dir,
            work_dir: dot_pixi_dir.join("build-v0"),
            tool_context,
            variant_config,
            git: GitResolver::default(),
        })
    }

    pub fn from_workspace(workspace: &Workspace) -> miette::Result<Self> {
        let variant = workspace.workspace.value.workspace.build_variants.clone();

        Self::new(
            get_cache_dir()?,
            workspace.pixi_dir(),
            workspace.channel_config(),
            variant,
            Arc::new(ToolContext::default()),
        )
        .into_diagnostic()
    }

    pub fn with_tool_context(self, tool_context: Arc<ToolContext>) -> Self {
        Self {
            tool_context,
            ..self
        }
    }

    /// Sets the input hash cache to use for caching input hashes.
    pub fn with_glob_hash_cache(self, glob_hash_cache: GlobHashCache) -> Self {
        Self {
            glob_hash_cache,
            ..self
        }
    }

    pub fn resolve_variant(&self, platform: Platform) -> HashMap<String, Vec<String>> {
        let mut result = HashMap::new();

        // Resolves from most specific to least specific.
        for variants in self.variant_config.resolve(Some(platform)).flatten() {
            // Update the hash map, but only items that are not already in the map.
            for (key, value) in variants {
                result.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }

        tracing::info!("resolved variant configuration: {:?}", result);

        result
    }

    /// Extracts the metadata for a package from the given source specification.
    #[allow(clippy::too_many_arguments)]
    pub async fn extract_source_metadata(
        &self,
        source_spec: &SourceSpec,
        channels: &[ChannelUrl],
        host_platform: Platform,
        host_virtual_packages: Vec<GenericVirtualPackage>,
        build_platform: Platform,
        build_virtual_packages: Vec<GenericVirtualPackage>,
        metadata_reporter: Arc<dyn BuildMetadataReporter>,
        source_reporter: Option<Arc<dyn SourceReporter>>,
        build_id: usize,
    ) -> Result<SourceMetadata, BuildError> {
        let source = self.fetch_source(source_spec, source_reporter).await?;
        let records = self
            .extract_records(
                &source,
                channels,
                host_platform,
                host_virtual_packages,
                build_platform,
                build_virtual_packages,
                metadata_reporter.clone(),
                build_id,
            )
            .await?;

        Ok(SourceMetadata { source, records })
    }

    /// Build a package from the given source specification.
    #[instrument(skip_all, fields(source = %source_spec.source))]
    #[allow(clippy::too_many_arguments)]
    pub async fn build_source_record(
        &self,
        source_spec: &SourceRecord,
        channels: &[ChannelUrl],
        host_platform: Platform,
        host_virtual_packages: Vec<GenericVirtualPackage>,
        build_virtual_packages: Vec<GenericVirtualPackage>,
        build_reporter: Arc<dyn BuildReporter>,
        source_reporter: Option<Arc<dyn SourceReporter>>,
        build_id: usize,
    ) -> Result<RepoDataRecord, BuildError> {
        let source_checkout = SourceCheckout {
            path: self
                .fetch_pinned_source(&source_spec.source, source_reporter)
                .await?,
            pinned: source_spec.source.clone(),
        };

        let channels_urls: Vec<Url> = channels.iter().cloned().map(Into::into).collect::<Vec<_>>();

        let (cached_build, entry) = self
            .build_cache
            .entry(
                &source_checkout,
                &BuildInput {
                    channel_urls: channels.iter().cloned().map(Into::into).collect(),
                    target_platform: Platform::from_str(&source_spec.package_record.subdir)
                        .ok()
                        .unwrap_or(host_platform),
                    name: source_spec.package_record.name.as_normalized().to_string(),
                    version: source_spec.package_record.version.to_string(),
                    build: source_spec.package_record.build.clone(),
                    host_platform,
                    host_virtual_packages: host_virtual_packages.clone(),
                    build_virtual_packages: build_virtual_packages.clone(),
                },
            )
            .await?;

        // Check if there are already cached builds
        if let Some(build) = cached_build {
            if let Some(record) = Self::cached_build_source_record(build, &source_checkout)? {
                build_reporter.on_build_cached(build_id);
                return Ok(record);
            }
        }

        let protocol = self.setup_protocol(&source_checkout, build_id).await?;

        // Build the package
        let build_result = protocol
            .conda_build(
                &CondaBuildParams {
                    host_platform: Some(PlatformAndVirtualPackages {
                        platform: host_platform,
                        virtual_packages: Some(host_virtual_packages.clone()),
                    }),
                    build_platform_virtual_packages: Some(build_virtual_packages.clone()),
                    channel_base_urls: Some(channels_urls),
                    channel_configuration: ChannelConfiguration {
                        base_url: self.channel_config.channel_alias.clone(),
                    },
                    // only use editable for build path dependencies
                    editable: source_spec.source.as_path().is_some(),
                    outputs: Some(vec![CondaOutputIdentifier {
                        name: Some(source_spec.package_record.name.as_normalized().to_string()),
                        version: Some(source_spec.package_record.version.version().to_string()),
                        build: Some(source_spec.package_record.build.clone()),
                        subdir: Some(source_spec.package_record.subdir.clone()),
                    }]),
                    work_directory: self.work_dir.join(
                        WorkDirKey {
                            source: source_checkout.clone(),
                            host_platform,
                            build_backend: protocol.identifier().to_string(),
                        }
                        .key(),
                    ),
                    variant_configuration: Some(self.resolve_variant(host_platform)),
                },
                build_reporter.as_conda_build_reporter(),
            )
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
            channel: None,
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
                        globs: protocol
                            .manifests()
                            .into_iter()
                            .chain(build_result.input_globs)
                            .collect_vec(),
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
        source_reporter: Option<Arc<dyn SourceReporter>>,
    ) -> Result<SourceCheckout, BuildError> {
        match source_spec {
            SourceSpec::Url(_) => unimplemented!("fetching URL sources is not yet implemented"),
            SourceSpec::Git(git_spec) => {
                let fetched = self
                    .resolve_git(
                        git_spec.clone(),
                        source_reporter.map(|sr| sr.as_git_reporter()),
                    )
                    .await
                    .map_err(|err| BuildError::FetchError(err.into()))?;
                //TODO: will be removed when manifest will be merged in pixi-build-backend
                let path = if let Some(subdir) = git_spec.subdirectory.as_ref() {
                    fetched.clone().into_path().join(subdir)
                } else {
                    fetched.clone().into_path()
                };

                let source_checkout = SourceCheckout {
                    path,
                    pinned: PinnedSourceSpec::Git(PinnedGitSpec {
                        git: fetched.git().repository().clone(),
                        source: PinnedGitCheckout {
                            commit: fetched.git().precise().expect("should be precies"),
                            reference: git_spec
                                .rev
                                .clone()
                                .unwrap_or(pixi_spec::GitReference::DefaultBranch),
                            subdirectory: git_spec.subdirectory.clone(),
                        },
                    }),
                };
                Ok(source_checkout)
            }
            SourceSpec::Path(path) => {
                let source_path = self
                    .resolve_path(path.path.to_path())
                    .map_err(|err| BuildError::ResolvePathSource(path.path.clone(), err))?;
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
        source_reporter: Option<Arc<dyn SourceReporter>>,
    ) -> Result<PathBuf, BuildError> {
        match source_spec {
            PinnedSourceSpec::Url(_) => {
                unimplemented!("fetching URL sources is not yet implemented")
            }
            PinnedSourceSpec::Git(pinned_git_spec) => {
                let fetched = self
                    .resolve_precise_git(
                        pinned_git_spec.clone(),
                        source_reporter.map(|sr| sr.as_git_reporter()),
                    )
                    .await
                    .map_err(|err| {
                        BuildError::ResolveGitSource(pinned_git_spec.git.clone(), err)
                    })?;
                let path = if let Some(subdir) = pinned_git_spec.source.subdirectory.as_ref() {
                    fetched.into_path().join(subdir)
                } else {
                    fetched.into_path()
                };
                Ok(path)
            }
            PinnedSourceSpec::Path(path) => self
                .resolve_path(path.path.to_path())
                .map_err(|err| BuildError::ResolvePathSource(path.path.clone(), err)),
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

    /// Resolves the source path to a full path.
    ///
    /// This function does not check if the path exists and also does not follow
    /// symlinks.
    async fn resolve_git(
        &self,
        git: GitSpec,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> miette::Result<Fetch> {
        let git_reference = git
            .rev
            .map(|rev| rev.into())
            .unwrap_or(GitReference::DefaultBranch);

        let git_url = GitUrl::try_from(git.git)
            .into_diagnostic()?
            .with_reference(git_reference);

        let resolver = self
            .git
            .fetch(
                &git_url,
                self.tool_context.clone().client.clone(),
                self.cache_dir.clone().join(CACHED_GIT_DIR),
                reporter,
            )
            .await
            .into_diagnostic()?;

        Ok(resolver)
    }

    /// Resolves the source path to a full path.
    ///
    /// This function does not check if the path exists and also does not follow
    /// symlinks.
    async fn resolve_precise_git(
        &self,
        git: PinnedGitSpec,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> miette::Result<Fetch> {
        let git_reference = git.source.reference.into();

        let git_url = GitUrl::from_commit(git.git, git_reference, git.source.commit);

        let resolver = self
            .git
            .fetch(
                &git_url,
                self.tool_context.clone().client.clone(),
                self.cache_dir.clone().join(CACHED_GIT_DIR),
                reporter,
            )
            .await
            .into_diagnostic()?;

        Ok(resolver)
    }

    /// Extracts the metadata from a package whose source is located at the
    /// given path.
    #[instrument(skip_all, fields(source = %source.pinned, platform = %host_platform))]
    #[allow(clippy::too_many_arguments)]
    async fn extract_records(
        &self,
        source: &SourceCheckout,
        channels: &[ChannelUrl],
        host_platform: Platform,
        host_virtual_packages: Vec<GenericVirtualPackage>,
        build_platform: Platform,
        build_virtual_packages: Vec<GenericVirtualPackage>,
        metadata_reporter: Arc<dyn BuildMetadataReporter>,
        build_id: usize,
    ) -> Result<Vec<SourceRecord>, BuildError> {
        let channel_urls = channels.iter().cloned().map(Into::into).collect::<Vec<_>>();
        let variant_configuration = self.resolve_variant(host_platform);

        let (cached_metadata, cache_entry) = self
            .source_metadata_cache
            .entry(
                source,
                &SourceMetadataInput {
                    channel_urls: channel_urls.clone(),
                    build_platform,
                    build_virtual_packages: build_virtual_packages.clone(),
                    host_platform,
                    host_virtual_packages: host_virtual_packages.clone(),
                    build_variants: variant_configuration.clone().into_iter().collect(),
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
                metadata_reporter.on_metadata_cached(build_id);
                // No input hash so just assume it is still valid.
                return Ok(source_metadata_to_records(
                    source,
                    metadata.packages,
                    metadata.input_hash,
                ));
            }
        }

        let protocol = self.setup_protocol(source, build_id).await?;

        // Extract the conda metadata for the package
        let metadata = protocol
            .conda_get_metadata(
                &CondaMetadataParams {
                    build_platform: Some(PlatformAndVirtualPackages {
                        platform: build_platform,
                        virtual_packages: Some(build_virtual_packages),
                    }),
                    host_platform: Some(PlatformAndVirtualPackages {
                        platform: host_platform,
                        virtual_packages: Some(host_virtual_packages),
                    }),
                    channel_base_urls: Some(channel_urls),
                    channel_configuration: ChannelConfiguration {
                        base_url: self.channel_config.channel_alias.clone(),
                    },
                    work_directory: self.work_dir.join(
                        WorkDirKey {
                            source: source.clone(),
                            host_platform,
                            build_backend: protocol.identifier().to_string(),
                        }
                        .key(),
                    ),
                    variant_configuration: Some(variant_configuration),
                },
                metadata_reporter.as_conda_metadata_reporter().clone(),
            )
            .await
            .map_err(|e| BuildError::BackendError(e.into()))?;

        // Compute the input globs for the mutable source checkouts.
        let input_hash = if source.pinned.is_immutable() {
            None
        } else {
            let input_globs = protocol
                .manifests()
                .into_iter()
                .chain(
                    metadata
                        .input_globs
                        .clone()
                        .into_iter()
                        .flat_map(|glob| glob.into_iter()),
                )
                .collect_vec();

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

    async fn setup_protocol(
        &self,
        source: &SourceCheckout,
        build_id: usize,
    ) -> Result<Protocol, BuildError> {
        // The RAYON_INITIALIZE is required to ensure that rayon is explicitly initialized.
        LazyLock::force(&RAYON_INITIALIZE);

        // Instantiate a protocol for the source directory.
        let protocol = pixi_build_frontend::BuildFrontend::default()
            .with_channel_config(self.channel_config.clone())
            .with_tool_context(self.tool_context.clone())
            .setup_protocol(SetupRequest {
                source_dir: source.path.clone(),
                build_tool_override: BackendOverride::from_env(),
                build_id,
            })
            .await
            .map_err(|frontend_error| {
                BuildError::BuildFrontendSetup(Box::new(source.clone()), frontend_error)
            })?;

        Ok(protocol)
    }

    fn cached_build_source_record(
        cached_build: CachedBuild,
        source_checkout: &SourceCheckout,
    ) -> Result<Option<RepoDataRecord>, BuildError> {
        // Check to see if the cached build is up-to-date.
        if let Some(source_input) = cached_build.source {
            let glob_time = GlobModificationTime::from_patterns(
                &source_checkout.path,
                source_input
                    .globs
                    .iter()
                    .map(String::as_str)
                    .chain(DEFAULT_BUILD_IGNORE_GLOBS.iter().copied()),
            )
            .map_err(BuildError::GlobModificationError)?;
            match glob_time {
                GlobModificationTime::MatchesFound {
                    modified_at,
                    designated_file,
                } => {
                    if cached_build
                        .record
                        .package_record
                        .timestamp
                        .map(|t| t >= chrono::DateTime::<Utc>::from(modified_at))
                        .unwrap_or(false)
                    {
                        tracing::debug!("found an up-to-date cached build.");
                        return Ok(Some(cached_build.record));
                    } else {
                        tracing::debug!(
                            "found an stale cached build, {} is newer than {}",
                            designated_file.display(),
                            cached_build
                                .record
                                .package_record
                                .timestamp
                                .unwrap_or_default()
                        );
                    }
                }
                GlobModificationTime::NoMatches => {
                    // No matches, so we should rebuild.
                    tracing::debug!("found a stale cached build, no files match the source glob");
                }
            }
        } else {
            tracing::debug!("found a cached build");
            // If there is no source info in the cache we assume its still valid.
            return Ok(Some(cached_build.record));
        }

        Ok(None)
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

/// A key to uniquely identify a work directory. If there is a source build with
/// the same key they will share the same working directory.
struct WorkDirKey {
    /// The location of the source
    source: SourceCheckout,

    /// The platform the dependency will run on
    host_platform: Platform,

    /// The build backend name
    /// TODO: Maybe we should also include the version.
    build_backend: String,
}

impl WorkDirKey {
    pub fn key(&self) -> String {
        let mut hasher = Xxh3::new();
        self.source.pinned.to_string().hash(&mut hasher);
        self.host_platform.hash(&mut hasher);
        self.build_backend.hash(&mut hasher);
        let unique_key = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
        match self.source.path.file_name().and_then(OsStr::to_str) {
            Some(name) => format!("{}-{}", name, unique_key),
            None => unique_key,
        }
    }
}
