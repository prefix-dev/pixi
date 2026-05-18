//! Compute-engine Key that builds a source record into a `.conda` artifact.
//! Build/host envs come pre-resolved on the input record, so no nested
//! solves run here. Caching is two-tiered: a content-addressed artifact
//! cache (hit returns the cached `.conda`) and a workspace cache that
//! gives the backend a stable incremental-build location across runs
//! sharing the same deps.

use std::{collections::BTreeMap, hash::Hash, path::PathBuf, sync::Arc};

use derive_more::Display;
use futures::{SinkExt, channel::mpsc::unbounded};
use pixi_build_types::procedures::conda_outputs::{CondaOutput, CondaOutputsParams};
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_record::{PixiRecord, UnresolvedPixiRecord, UnresolvedSourceRecord, VariantValue};
use pixi_spec::{ResolvedExcludeNewer, SourceAnchor, SourceLocationSpec};
use pixi_variant::VariantSelector;
use rattler_conda_types::{
    ChannelUrl, PackageRecord, RepoDataRecord, package::DistArchiveIdentifier, prefix::Prefix,
};
use rattler_digest::Sha256Hash;
use tracing::instrument;
use url::Url;

pub use crate::cache::{ArtifactCache, WorkspaceCache};
use crate::cache::{
    ArtifactCacheError, compute_artifact_cache_key, compute_workspace_key,
    markers::{SourceBuildArtifactsDir, SourceBuildWorkspacesDir},
};
use crate::{
    BackendSourceBuildError, BackendSourceBuildExt, BackendSourceBuildMethod,
    BackendSourceBuildPrefix, BackendSourceBuildSpec, BackendSourceBuildV1Method, BuildEnvironment,
    BuildProfile, CommandDispatcherError, CommandDispatcherErrorResultExt,
    InstallPixiEnvironmentExt, InstallPixiEnvironmentSpec, InstantiateBackendKey,
    ProjectModelOverrides, SourceBuildError,
    build::{Dependencies, PixiRunExports},
};
use pixi_compute_cache_dirs::CacheDirsExt;
use pixi_compute_sources::SourceCheckoutExt;

/// Unwrap a `CommandDispatcherError<E>` produced by a `ctx.*` ext call.
/// Cancellation is handled at the engine layer, so it shouldn't reach
/// here; treat it as a programming error.
fn unwrap_dispatcher_err<E>(err: CommandDispatcherError<E>) -> E {
    match err {
        CommandDispatcherError::Failed(e) => e,
        CommandDispatcherError::Cancelled => {
            unreachable!("compute-engine cancellation does not surface inside a Key compute body")
        }
    }
}

/// Hashable inputs to a source build. Runtime concerns (reporters, log
/// sinks, force-rebuild) stay out of the spec; force-rebuild wipes the
/// artifact-cache entry before calling the Key.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SourceBuildSpec {
    /// `build_packages` and `host_packages` are expected to be populated
    /// upstream.
    pub record: Arc<UnresolvedSourceRecord>,

    pub channels: Vec<ChannelUrl>,

    pub exclude_newer: Option<ResolvedExcludeNewer>,

    pub build_environment: BuildEnvironment,

    pub build_profile: BuildProfile,

    pub variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,

    pub variant_files: Option<Vec<PathBuf>>,

    /// User-supplied build string prefix forwarded to the backend's
    /// project model. Overrides any value declared in the manifest.
    pub build_string_prefix: Option<String>,

    /// User-supplied build number forwarded to the backend's project
    /// model. Overrides any value declared in the manifest.
    pub build_number: Option<u64>,
}

/// Built artifact plus its sha256 and a
/// [`RepoDataRecord`]. `artifact_sha256` propagates into dependents'
/// cache keys for transitive invalidation.
#[derive(Debug, Clone)]
pub struct SourceBuildResult {
    pub artifact: PathBuf,

    pub artifact_sha256: Sha256Hash,

    /// `url` points at [`Self::artifact`]; `sha256` matches
    /// [`Self::artifact_sha256`].
    pub record: RepoDataRecord,
}

#[derive(Clone, Debug, Display, Eq, Hash, PartialEq)]
#[display("{}", _0.record.name().as_source())]
pub struct SourceBuildKey(pub Arc<SourceBuildSpec>);

impl SourceBuildKey {
    pub fn new(spec: SourceBuildSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Key for SourceBuildKey {
    type Value = Result<Arc<SourceBuildResult>, SourceBuildError>;

    #[instrument(
        skip_all,
        name = "source-build",
        fields(
            package = %self.0.record.name().as_source(),
            host_platform = %self.0.build_environment.host_platform,
        )
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = self.0.clone();
        compute_inner(ctx, spec).await.map(Arc::new)
    }
}

/// Core body of [`SourceBuildKey::compute`]. Separated out to keep error
/// mapping + reporter scaffolding orthogonal to the pipeline itself.
async fn compute_inner(
    ctx: &mut ComputeCtx,
    spec: Arc<SourceBuildSpec>,
) -> Result<SourceBuildResult, SourceBuildError> {
    // sha256s are collected in a stable (build, host) order so the
    // artifact cache key stays deterministic across buckets.
    let (build_source_dep_sha256s, host_source_dep_sha256s) =
        recurse_source_deps(ctx, &spec).await?;

    let manifest_source = spec.record.manifest_source.clone();
    let manifest_checkout = ctx
        .checkout_pinned_source(manifest_source.clone())
        .await
        .map_err(SourceBuildError::SourceCheckout)?;
    let build_source_checkout = match spec.record.build_source.as_ref() {
        Some(pinned) => ctx
            .checkout_pinned_source(pinned.pinned().clone())
            .await
            .map_err(SourceBuildError::SourceCheckout)?,
        None => manifest_checkout.clone(),
    };

    // Resolve a stable backend identifier WITHOUT spawning the JSON-RPC
    // backend. Goes through the same dependency-resolution Keys
    // (`DiscoveredBackendKey` -> `ResolvedBackendCommandKey` ->
    // `EphemeralEnvKey` for env-spec backends) so when a real
    // instantiation runs later in the same process the work is shared
    // via the engine's dedup; what's skipped here is the spawn,
    // JSON-RPC handshake, and activator run.
    //
    // We need this identifier only to form the artifact cache key — on
    // a cache hit there's no further reason to talk to a backend, so
    // doing the spawn before the lookup is pure waste.
    let manifest_anchor = SourceAnchor::from(SourceLocationSpec::from(manifest_source.clone()));
    let build_source_dir = build_source_checkout
        .path
        .as_dir_or_file_parent()
        .to_path_buf();
    let backend_identifier = crate::resolve_backend_identifier(
        ctx,
        manifest_checkout.path.as_std_path(),
        manifest_anchor.clone(),
        spec.exclude_newer.clone(),
    )
    .await
    .map_err(|err: Arc<crate::InstantiateBackendError>| {
        SourceBuildError::Initialize((*err).clone())
    })?;

    // Cache key covers structural identity + dep content addresses;
    // source-file freshness lives in the sidecar, not the key.
    let project_model_overrides = ProjectModelOverrides {
        build_string_prefix: spec.build_string_prefix.clone(),
        build_number: spec.build_number,
    };
    let cache_key = compute_artifact_cache_key(
        &spec.record,
        spec.build_environment.build_platform,
        spec.build_environment.host_platform,
        &backend_identifier,
        &build_source_dep_sha256s,
        &host_source_dep_sha256s,
        &project_model_overrides,
    );

    // On artifact cache hit, return without invoking the backend.
    // Force-rebuild is handled by wiping the cache entry before calling;
    // this body honors whatever state it finds on disk.
    let artifacts_dir = ctx.cache_dir::<SourceBuildArtifactsDir>().await;
    let artifact_cache = ArtifactCache::new(artifacts_dir.as_std_path());
    let source_dir = build_source_checkout
        .path
        .as_dir_or_file_parent()
        .as_std_path()
        .to_path_buf();
    if let Some(hit) = artifact_cache
        .lookup(spec.record.name(), &cache_key, &source_dir)
        .await
        .map_err(map_cache_err)?
    {
        tracing::debug!(
            package = %spec.record.name().as_source(),
            artifact = %hit.artifact.display(),
            "artifact cache hit",
        );
        return Ok(SourceBuildResult {
            artifact: hit.artifact,
            artifact_sha256: hit.sha256,
            record: hit.record,
        });
    }

    // Cache miss: now spawn the backend. `InstantiateBackendKey`
    // re-uses the discovery / ephemeral-env work the identifier
    // resolve already cached, so this only pays for the JSON-RPC
    // spawn + handshake + activator.
    let backend = ctx
        .compute(
            &InstantiateBackendKey::new(
                manifest_checkout.path.as_std_path(),
                manifest_anchor.clone(),
                build_source_dir,
                spec.exclude_newer.clone(),
            )
            .with_project_model_overrides(project_model_overrides),
        )
        .await
        .map_err(|err: Arc<crate::InstantiateBackendError>| {
            SourceBuildError::Initialize((*err).clone())
        })?;

    // Workspace dir is the backend's build root; state persists across
    // runs that share the same (source, deps, variants, backend).
    let workspace_key = compute_workspace_key(
        &spec.record,
        spec.build_environment.build_platform,
        spec.build_environment.host_platform,
        &backend_identifier,
    );
    let workspaces_dir = ctx.cache_dir::<SourceBuildWorkspacesDir>().await;
    let workspace_cache = WorkspaceCache::new(workspaces_dir.as_std_path());
    // ensure_dir_locked holds an exclusive cross-process lock for the
    // guard's lifetime, so a concurrent pixi process building the same
    // (source, deps, variants, backend) combination blocks here.
    let workspace_guard = workspace_cache
        .ensure_dir_locked(spec.record.name(), &workspace_key)
        .await
        .map_err(|err| SourceBuildError::CreateWorkDirectory(Arc::new(err)))?;
    let work_directory = workspace_guard.path().to_path_buf();

    // Find the output matching our (name, variants) pair. Caching this
    // call is future work; see BuildBackendMetadataKey for a shape that
    // could dedup.
    let output = fetch_matching_output(&backend, &spec, &work_directory).await?;

    // install_prefix recurses into source entries via SourceBuildKey,
    // so build_records / host_records are all binaries on disk.
    let directories = Directories::new(&work_directory, spec.build_environment.host_platform);
    let (build_records, _build_install_result) = install_prefix(
        ctx,
        &spec,
        InstallTarget::Build,
        directories.build_prefix.clone(),
        spec.record.build_packages.clone(),
    )
    .await?;

    let (host_records, _host_install_result) = install_prefix(
        ctx,
        &spec,
        InstallTarget::Host,
        directories.host_prefix.clone(),
        spec.record.host_packages.clone(),
    )
    .await?;

    // Resolve `pin_compatible` markers against the build/host records we
    // just produced. Visibility ordering:
    // - build deps see no compat records (can't pin_compatible inside
    //   the env being defined)
    // - host deps see build records
    // - run deps (+ run_exports) see build + host records
    let source_anchor = SourceAnchor::from(SourceLocationSpec::from(manifest_source.clone()));
    let build_pixi_records: Vec<PixiRecord> = build_records
        .iter()
        .cloned()
        .map(|r| PixiRecord::Binary(Arc::new(r)))
        .collect();
    let host_pixi_records: Vec<PixiRecord> = host_records
        .iter()
        .cloned()
        .map(|r| PixiRecord::Binary(Arc::new(r)))
        .collect();

    let build_dependencies = output
        .build_dependencies
        .as_ref()
        .map(|deps| {
            Dependencies::new(
                deps,
                Some(source_anchor.clone()),
                &std::collections::HashMap::new(),
            )
        })
        .transpose()
        .map_err(SourceBuildError::from)?
        .unwrap_or_default();

    let mut compat_map: std::collections::HashMap<rattler_conda_types::PackageName, &PixiRecord> =
        std::collections::HashMap::new();
    for r in &build_pixi_records {
        compat_map.insert(r.name().clone(), r);
    }

    let host_dependencies = output
        .host_dependencies
        .as_ref()
        .map(|deps| Dependencies::new(deps, Some(source_anchor.clone()), &compat_map))
        .transpose()
        .map_err(SourceBuildError::from)?
        .unwrap_or_default();

    for r in &host_pixi_records {
        compat_map.insert(r.name().clone(), r);
    }

    let run_dependencies = Dependencies::new(&output.run_dependencies, None, &compat_map)
        .map_err(SourceBuildError::from)?;
    let run_exports = PixiRunExports::try_from_protocol(&output.run_exports, &compat_map)
        .map_err(SourceBuildError::from)?;

    let editable =
        matches!(spec.build_profile, BuildProfile::Development) && spec.record.has_mutable_source();
    let built = ctx
        .backend_source_build(BackendSourceBuildSpec {
            method: BackendSourceBuildMethod::BuildV1(BackendSourceBuildV1Method {
                editable,
                dependencies: run_dependencies,
                run_exports,
                build_prefix: BackendSourceBuildPrefix {
                    platform: spec.build_environment.build_platform,
                    prefix: directories.build_prefix,
                    dependencies: build_dependencies,
                    records: build_records,
                },
                host_prefix: BackendSourceBuildPrefix {
                    platform: spec.build_environment.host_platform,
                    prefix: directories.host_prefix,
                    dependencies: host_dependencies,
                    records: host_records,
                },
                variant: output.metadata.variant.clone(),
                output_directory: None,
            }),
            backend,
            name: output.metadata.name.clone(),
            version: output.metadata.version.clone(),
            build: output.metadata.build.clone(),
            subdir: output.metadata.subdir.to_string(),
            source_dir: source_dir.clone(),
            work_directory,
            channels: spec.channels.clone(),
        })
        .await
        .map_err_with(SourceBuildError::from)
        .map_err(unwrap_dispatcher_err)?;

    // Synthesize a RepoDataRecord from the built .conda so the cache
    // can persist it alongside the artifact.
    let record = synthesize_repodata(&built.output_file).await?;

    let stored = artifact_cache
        .store(
            spec.record.name(),
            &cache_key,
            &built.output_file,
            built.input_globs,
            built.input_files,
            &source_dir,
            record,
        )
        .await
        .map_err(map_cache_err)?;

    Ok(SourceBuildResult {
        artifact: stored.artifact,
        artifact_sha256: stored.sha256,
        record: stored.record,
    })
}

/// Fan out over every source entry in `build_packages` and
/// `host_packages`, recursively build each via [`SourceBuildKey`], and
/// return their sha256s split by bucket. The two buckets feed into the
/// cache key separately so a dep moving build ↔ host invalidates.
async fn recurse_source_deps(
    ctx: &mut ComputeCtx,
    spec: &Arc<SourceBuildSpec>,
) -> Result<(Vec<Sha256Hash>, Vec<Sha256Hash>), SourceBuildError> {
    // build_packages run on the build platform. The nested build's
    // HOST platform is therefore the outer's BUILD platform.
    let build = build_source_deps(
        ctx,
        spec.clone(),
        spec.record.build_packages.clone(),
        spec.build_environment.to_build_from_build(),
    )
    .await?;
    // host_packages target the outer host platform. The nested build's
    // build_environment matches the outer's.
    let host = build_source_deps(
        ctx,
        spec.clone(),
        spec.record.host_packages.clone(),
        spec.build_environment.clone(),
    )
    .await?;
    Ok((build, host))
}

/// Build a single bucket (build or host) of source dependencies concurrently.
async fn build_source_deps(
    ctx: &mut ComputeCtx,
    spec: Arc<SourceBuildSpec>,
    packages: Vec<UnresolvedPixiRecord>,
    nested_build_environment: BuildEnvironment,
) -> Result<Vec<Sha256Hash>, SourceBuildError> {
    let sources: Vec<Arc<UnresolvedSourceRecord>> = packages
        .into_iter()
        .filter_map(|r| match r {
            UnresolvedPixiRecord::Source(s) => Some(s),
            UnresolvedPixiRecord::Binary(_) => None,
        })
        .collect();
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let mapper = {
        let spec = spec.clone();
        let build_env = nested_build_environment;
        async move |sub_ctx: &mut ComputeCtx,
                    src: Arc<UnresolvedSourceRecord>|
                    -> Result<Sha256Hash, SourceBuildError> {
            let nested_spec = SourceBuildSpec {
                record: src,
                channels: spec.channels.clone(),
                exclude_newer: spec.exclude_newer.clone(),
                build_environment: build_env.clone(),
                build_profile: spec.build_profile,
                variant_configuration: spec.variant_configuration.clone(),
                variant_files: spec.variant_files.clone(),
                // Nested source builds inherit the user-supplied
                // overrides from the top-level invocation so the entire
                // dependency closure builds against consistent values.
                build_string_prefix: spec.build_string_prefix.clone(),
                build_number: spec.build_number,
            };
            let result = sub_ctx.compute(&SourceBuildKey::new(nested_spec)).await?;
            Ok(result.artifact_sha256)
        }
    };
    ctx.try_compute_join(sources, mapper).await
}

/// Call `conda_outputs` on the backend and pick the one matching this
/// record's name + variants.
async fn fetch_matching_output(
    backend: &crate::BackendHandle,
    spec: &SourceBuildSpec,
    work_directory: &std::path::Path,
) -> Result<CondaOutput, SourceBuildError> {
    let variant_config = spec.variant_configuration.as_ref().map(|variants| {
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
    });

    // The backend streams log lines; we drop them here (no SourceBuild
    // reporter lifecycle is wired up yet).
    let (mut log_sink, _log_rx) = unbounded::<String>();
    let outputs = backend
        .lock()
        .await
        .conda_outputs(
            CondaOutputsParams {
                host_platform: spec.build_environment.host_platform,
                build_platform: spec.build_environment.build_platform,
                variant_configuration: variant_config,
                variant_files: spec.variant_files.clone(),
                work_directory: work_directory.to_path_buf(),
                channels: spec.channels.clone(),
            },
            move |line| {
                let _ = futures::executor::block_on(log_sink.send(line));
            },
        )
        .await
        .map_err(BackendSourceBuildError::from)
        .map_err(SourceBuildError::from)?;

    let selector = VariantSelector::new(
        spec.record
            .variants
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let matching = outputs
        .outputs
        .into_iter()
        .filter(|o| &o.metadata.name == spec.record.name());
    selector
        .find(matching, |o| &o.metadata.variant)
        .ok_or_else(|| SourceBuildError::MissingOutput {
            name: spec.record.name().as_normalized().to_string(),
            variants: spec
                .record
                .variants
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
}

#[derive(Copy, Clone)]
enum InstallTarget {
    Build,
    Host,
}

/// Install a build or host environment into `prefix`, returning the
/// fully-resolved `RepoDataRecord`s that end up inside.
async fn install_prefix(
    ctx: &mut ComputeCtx,
    spec: &SourceBuildSpec,
    target: InstallTarget,
    prefix_path: PathBuf,
    packages: Vec<UnresolvedPixiRecord>,
) -> Result<
    (
        Vec<RepoDataRecord>,
        Option<crate::InstallPixiEnvironmentResult>,
    ),
    SourceBuildError,
> {
    // Always create the prefix directory, even when empty. The backend's
    // build script expands $PREFIX into this path and writes into it, so
    // it must exist on disk regardless of whether any packages got
    // installed.
    let prefix = Prefix::create(&prefix_path)
        .map_err(|e| SourceBuildError::CreateBuildEnvironmentDirectory(Arc::new(e)))?;
    if packages.is_empty() {
        return Ok((Vec::new(), None));
    }
    let build_environment = match target {
        InstallTarget::Build => spec.build_environment.to_build_from_build(),
        InstallTarget::Host => spec.build_environment.clone(),
    };
    let label = match target {
        InstallTarget::Build => format!("{} (build)", spec.record.name().as_source()),
        InstallTarget::Host => format!("{} (host)", spec.record.name().as_source()),
    };
    let install_spec = InstallPixiEnvironmentSpec {
        name: label,
        records: packages.clone(),
        prefix,
        installed: None,
        ignore_packages: None,
        build_environment,
        force_reinstall: Default::default(),
        exclude_newer: spec.exclude_newer.clone(),
        channels: spec.channels.clone(),
        variant_configuration: spec.variant_configuration.clone(),
        variant_files: spec.variant_files.clone(),
    };
    let result = ctx
        .install_pixi_environment(install_spec)
        .await
        .map_err_with(|e| match target {
            InstallTarget::Build => SourceBuildError::InstallBuildEnvironment(Arc::new(e)),
            InstallTarget::Host => SourceBuildError::InstallHostEnvironment(Arc::new(e)),
        })
        .map_err(unwrap_dispatcher_err)?;

    // Collect the RepoDataRecords that were installed: binaries pass
    // through, sources come from the resolved_source_records map the
    // ctx install just populated.
    let mut records = Vec::with_capacity(packages.len());
    for r in packages {
        match r {
            UnresolvedPixiRecord::Binary(rec) => records.push((*rec).clone()),
            UnresolvedPixiRecord::Source(src) => {
                let built = result
                    .resolved_source_records
                    .get(src.name())
                    .cloned()
                    .expect(
                        "source package should have been built by ctx.install_pixi_environment",
                    );
                records.push((*built).clone());
            }
        }
    }
    Ok((records, Some(result)))
}

/// Read index.json out of the freshly-built `.conda` and synthesize a
/// `RepoDataRecord` for it.
async fn synthesize_repodata(
    output_file: &std::path::Path,
) -> Result<RepoDataRecord, SourceBuildError> {
    let file_name = output_file
        .file_name()
        .expect("backend did not return a file name")
        .to_string_lossy()
        .into_owned();
    let identifier = DistArchiveIdentifier::try_from_filename(&file_name)
        .expect("backend returned an invalid archive filename");
    let sha = compute_package_sha256(output_file).await?;
    let path = output_file.to_path_buf();
    let index_json = tokio::task::spawn_blocking(move || {
        rattler_package_streaming::seek::read_package_file(&path)
    })
    .await
    .expect("index.json read task panicked")
    .map_err(|err| SourceBuildError::ReadIndexJson(Arc::new(err)))?;
    let package_record = PackageRecord::from_index_json(index_json, None, Some(sha), None)
        .map_err(|err| SourceBuildError::ConvertSubdir(Arc::new(err)))?;
    Ok(RepoDataRecord {
        package_record,
        identifier,
        url: Url::from_file_path(output_file).expect("the output file should be a valid URL"),
        channel: None,
    })
}

/// Compute the sha256 of a file on a blocking thread.
async fn compute_package_sha256(path: &std::path::Path) -> Result<Sha256Hash, SourceBuildError> {
    let p = path.to_path_buf();
    tokio::task::spawn_blocking({
        let p = p.clone();
        move || rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&p)
    })
    .await
    .expect("sha256 task panicked")
    .map_err(|e| SourceBuildError::CalculateSha256(p, Arc::new(e)))
}

fn map_cache_err(err: ArtifactCacheError) -> SourceBuildError {
    match err {
        ArtifactCacheError::Io {
            operation,
            path,
            source,
        } => {
            let msg = format!("{operation} at {}", path.display());
            SourceBuildError::CreateWorkDirectory(Arc::new(std::io::Error::new(source.kind(), msg)))
        }
        ArtifactCacheError::Glob(err) => SourceBuildError::GlobSet(err),
        ArtifactCacheError::ArtifactFilename(path) => SourceBuildError::MissingOutputFile(path),
    }
}

/// Build/host prefixes for a source build: both sit under the workspace
/// dir, with platform-specific padding for non-Windows hosts.
struct Directories {
    build_prefix: PathBuf,
    host_prefix: PathBuf,
}

impl Directories {
    fn new(work_directory: &std::path::Path, host_platform: rattler_conda_types::Platform) -> Self {
        const BUILD_DIR: &str = "bld";
        const HOST_ENV_DIR: &str = "host";
        const PLACEHOLDER_TEMPLATE_STR: &str = "_placehold";

        let build_prefix = work_directory.join(BUILD_DIR);
        let host_prefix = if host_platform.is_windows() {
            work_directory.join(HOST_ENV_DIR)
        } else {
            // Non-Windows backends expect a 255-char host prefix for
            // reliable prefix replacement. Pad with a template string.
            const PLACEHOLDER_LENGTH: usize = 255;
            let mut placeholder = String::new();
            while placeholder.len() < PLACEHOLDER_LENGTH {
                placeholder.push_str(PLACEHOLDER_TEMPLATE_STR);
            }
            let placeholder = placeholder
                [0..PLACEHOLDER_LENGTH - work_directory.join(HOST_ENV_DIR).as_os_str().len()]
                .to_string();
            work_directory.join(format!("{HOST_ENV_DIR}{placeholder}"))
        };
        Self {
            build_prefix,
            host_prefix,
        }
    }
}
