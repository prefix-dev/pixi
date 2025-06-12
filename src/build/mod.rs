mod cache;
use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsStr,
    hash::{Hash, Hasher},
    ops::Not,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, LazyLock},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic};
use pixi_build_frontend::{BackendOverride, JsonRPCBuildProtocol, SetupRequest, ToolContext};
use pixi_build_types::{
    ChannelConfiguration, PlatformAndVirtualPackages, SourcePackageSpecV1,
    procedures::conda_build::{CondaBuildParams, CondaOutputIdentifier},
};
use pixi_command_dispatcher::{
    BuildEnvironment, CommandDispatcher, CommandDispatcherError, SourceCheckout,
    SourceCheckoutError, SourceMetadata, SourceMetadataSpec,
};
use pixi_git::GitError;
pub use pixi_glob::{GlobHashCache, GlobHashError};
use pixi_glob::{GlobModificationTime, GlobModificationTimeError};
use pixi_manifest::Targets;
use pixi_record::SourceRecord;
use pixi_spec::SourceSpec;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, GenericVirtualPackage, Platform, RepoDataRecord,
};
use rattler_digest::Sha256;
use thiserror::Error;
use tracing::instrument;
use typed_path::Utf8TypedPathBuf;
use url::Url;
use uv_configuration::RAYON_INITIALIZE;
use xxhash_rust::xxh3::Xxh3;

use crate::{
    Workspace,
    build::cache::{BuildCache, BuildInput, CachedBuild, SourceInfo},
    reporters::{BuildMetadataReporter, BuildReporter},
};

/// A list of globs that should be ignored when calculating any input hash.
/// These are typically used for build artifacts that should not be included in
/// the input hash.
const DEFAULT_BUILD_IGNORE_GLOBS: &[&str] = &["!.pixi/**"];

/// The [`BuildContext`] is used to build packages from source.
#[derive(Clone)]
pub struct BuildContext {
    channel_config: ChannelConfig,
    build_cache: BuildCache,
    work_dir: PathBuf,
    tool_context: Arc<ToolContext>,
    variant_config: Targets<Option<HashMap<String, Vec<String>>>>,
    command_dispatcher: CommandDispatcher,
}

#[derive(Debug, Error, Diagnostic)]
pub enum BuildError {
    #[error("failed to resolve path source {}", &.0)]
    ResolvePathSource(Utf8TypedPathBuf, #[source] std::io::Error),

    #[error("failed to resolve git source {}", &.0)]
    ResolveGitSource(Url, #[diagnostic_source] miette::Report),

    #[error("error calculating sha for {}", &.0.display())]
    CalculateSha(PathBuf, #[source] std::io::Error),

    #[error("the initialization of the build backend for '{}' failed", &.0.pinned)]
    BuildFrontendSetup(
        Box<SourceCheckout>,
        #[source]
        #[diagnostic_source]
        pixi_build_frontend::BuildFrontendError,
    ),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    FrontendError(Box<dyn Diagnostic + Send + Sync + 'static>),

    #[error("failed to determine metadata of source dependency '{}'", .0.as_source())]
    SourceMetadataError2(
        rattler_conda_types::PackageName,
        #[source]
        #[diagnostic_source]
        CommandDispatcherError<pixi_command_dispatcher::SourceMetadataError>,
    ),

    #[error(transparent)]
    InputHash(#[from] GlobHashError),

    #[error(transparent)]
    GlobModificationError(#[from] GlobModificationTimeError),

    #[error(transparent)]
    BuildCacheError(#[from] cache::BuildCacheError),

    #[error(transparent)]
    BuildFolderNotWritable(#[from] std::io::Error),

    #[error(transparent)]
    GitFetch(#[from] GitError),

    #[error(transparent)]
    SourceCheckoutError(#[from] CommandDispatcherError<SourceCheckoutError>),
}

impl BuildContext {
    pub fn new(
        channel_config: ChannelConfig,
        variant_config: Targets<Option<HashMap<String, Vec<String>>>>,
        command_dispatcher: CommandDispatcher,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            channel_config,
            build_cache: BuildCache::new(command_dispatcher.cache_dirs().source_builds()),
            work_dir: command_dispatcher.cache_dirs().working_dirs(),
            tool_context: Arc::new(
                ToolContext::builder()
                    .with_cache_dir(command_dispatcher.cache_dirs().build_backends())
                    .with_client(command_dispatcher.download_client().clone())
                    .with_gateway(command_dispatcher.gateway().clone())
                    .build(),
            ),
            variant_config,
            command_dispatcher,
        })
    }

    pub fn from_workspace(
        workspace: &Workspace,
        command_dispatcher: CommandDispatcher,
    ) -> miette::Result<Self> {
        let variant = workspace.workspace.value.workspace.build_variants.clone();
        Self::new(workspace.channel_config(), variant, command_dispatcher).into_diagnostic()
    }

    pub fn command_dispatcher(&self) -> &CommandDispatcher {
        &self.command_dispatcher
    }

    pub fn channel_config(&self) -> &ChannelConfig {
        &self.channel_config
    }

    pub fn with_tool_context(self, tool_context: Arc<ToolContext>) -> Self {
        Self {
            tool_context,
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

        tracing::trace!("resolved variant configuration: {:?}", result);

        result
    }

    /// Extracts the metadata for a package from the given source specification.
    #[allow(clippy::too_many_arguments)]
    pub async fn extract_source_metadata(
        &self,
        package_name: &rattler_conda_types::PackageName,
        source_spec: &SourceSpec,
        channels: &[ChannelUrl],
        build_environment: BuildEnvironment,
        _metadata_reporter: Arc<dyn BuildMetadataReporter>,
        _build_id: usize,
    ) -> Result<Arc<SourceMetadata>, BuildError> {
        let source = self
            .command_dispatcher
            .pin_and_checkout(source_spec.clone())
            .await
            .map_err(BuildError::SourceCheckoutError)?;

        self.command_dispatcher
            .source_metadata(SourceMetadataSpec {
                source,
                channel_config: self.channel_config.clone(),
                channels: channels.to_vec(),
                variants: Some(
                    self.resolve_variant(build_environment.host_platform)
                        .into_iter()
                        .collect(),
                ),
                build_environment,
                enabled_protocols: Default::default(),
            })
            .await
            .map_err(|error| BuildError::SourceMetadataError2(package_name.clone(), error))
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
        build_id: usize,
        rebuild: bool,
    ) -> Result<RepoDataRecord, BuildError> {
        let source_checkout = self
            .command_dispatcher
            .checkout_pinned_source(source_spec.source.clone())
            .await?;

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
        if !rebuild {
            if let Some(build) = cached_build {
                if let Some(record) = Self::cached_build_source_record(build, &source_checkout)? {
                    build_reporter.on_build_cached(build_id);
                    return Ok(record);
                }
            }
        }

        let protocol = self.setup_protocol(&source_checkout, build_id).await?;

        let mut outputs = BTreeSet::new();
        outputs.insert(CondaOutputIdentifier {
            name: Some(source_spec.package_record.name.as_normalized().to_string()),
            version: Some(source_spec.package_record.version.version().to_string()),
            build: Some(source_spec.package_record.build.clone()),
            subdir: Some(source_spec.package_record.subdir.clone()),
        });

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
                    outputs: Some(outputs),
                    work_directory: self.work_dir.join(
                        WorkDirKey {
                            source: source_checkout.clone(),
                            host_platform,
                            build_backend: protocol.backend_identifier().to_string(),
                        }
                        .key(),
                    ),
                    variant_configuration: Some(self.resolve_variant(host_platform)),
                },
                build_reporter.as_conda_build_reporter().as_ref(),
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
                            .collect(),
                    }),
                record: record.clone(),
            })
            .await?;

        Ok(updated_record)
    }

    async fn setup_protocol(
        &self,
        source: &SourceCheckout,
        build_id: usize,
    ) -> Result<JsonRPCBuildProtocol, BuildError> {
        // The RAYON_INITIALIZE is required to ensure that rayon is explicitly
        // initialized.
        LazyLock::force(&RAYON_INITIALIZE);

        // Instantiate a protocol for the source directory.
        let protocol = pixi_build_frontend::BuildFrontend::default()
            .with_channel_config(self.channel_config.clone())
            .with_tool_context(self.tool_context.clone())
            .setup_protocol(SetupRequest {
                source_dir: source.path.clone(),
                build_tool_override: BackendOverride::from_env()
                    .map_err(|e| BuildError::BackendError(e.into()))?,
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
                pixi_build_types::GitReferenceV1::Branch(b) => pixi_spec::GitReference::Branch(b),
                pixi_build_types::GitReferenceV1::Tag(t) => pixi_spec::GitReference::Tag(t),
                pixi_build_types::GitReferenceV1::Rev(rev) => pixi_spec::GitReference::Rev(rev),
                pixi_build_types::GitReferenceV1::DefaultBranch => {
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

/// A key to uniquely identify a work directory. If there is a source build with
/// the same key they will share the same working directory.
pub(crate) struct WorkDirKey {
    /// The location of the source
    source: SourceCheckout,

    /// The platform the dependency will run on
    host_platform: Platform,

    /// The build backend name
    /// TODO: Maybe we should also include the version.
    build_backend: String,
}

impl WorkDirKey {
    pub fn new(source: SourceCheckout, host_platform: Platform, build_backend: String) -> Self {
        Self {
            source,
            host_platform,
            build_backend,
        }
    }

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
