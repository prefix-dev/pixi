//! Content-addressed compute-engine Key for disposable, binary-only conda
//! environments. Source specs are rejected up-front. The prefix path is
//! derived from the spec hash and locked with [`AsyncPrefixGuard`] for
//! cross-process safety.
//!
//! This key has no per-key reporter trait of its own. The ephemeral
//! prefix is populated as part of backend instantiation, so it reads
//! [`InstantiateBackendReporter`](crate::reporter::InstantiateBackendReporter)
//! from the engine `DataStore` to build the rattler installer's
//! per-call reporter.

use std::{
    collections::BTreeMap,
    fmt,
    hash::{Hash, Hasher},
    mem,
    path::PathBuf,
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::TryFutureExt;
use itertools::Either;
use miette::Diagnostic;
use pixi_compute_engine::{ComputeCtx, DataStore, Key};
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, PixiSpec, ResolvedExcludeNewer};
use pixi_spec_containers::DependencyMap;
use pixi_utils::AsyncPrefixGuard;
use rattler::install::InstallerError;
use rattler_conda_types::{ChannelUrl, PackageName, RepoDataRecord, prefix::Prefix};
use rattler_repodata_gateway::{GatewayError, RepoData};
use rattler_solve::{ChannelPriority, SolveStrategy};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

use crate::SolveCondaEnvironmentSpec;
use crate::cache::markers::BuildBackendsDir;
use crate::compute_data::{HasCondaSolveReporter, HasGateway, HasInstantiateBackendReporter};
use crate::injected_config::{ChannelConfigKey, ToolBuildEnvironmentKey};
use crate::install_binary::install_binary_records;
use crate::reporter::{InstantiateBackendReporter, WrappingGatewayReporter};
use crate::solve_binary::SolveCondaExt;
use crate::solve_conda::SolveCondaEnvironmentError;
use pixi_compute_cache_dirs::CacheDirsExt;

/// Specification for an ephemeral, binary-only conda environment.
///
/// The spec's hash is used as the prefix cache key; callers with an
/// equal spec share the same installed prefix. Source `PixiSpec`s in
/// `dependencies` cause the Key to fail at compute time.
#[derive(Clone, Debug)]
pub struct EphemeralEnvSpec {
    /// Package requirements. Must be binary-only at compute time.
    pub dependencies: DependencyMap<PackageName, PixiSpec>,

    /// Additional solver constraints.
    pub constraints: DependencyMap<PackageName, BinarySpec>,

    /// Channels to search.
    pub channels: Vec<ChannelUrl>,

    /// Exclude-newer cutoff.
    pub exclude_newer: Option<ResolvedExcludeNewer>,

    /// Solver strategy.
    pub strategy: SolveStrategy,

    /// Channel priority.
    pub channel_priority: ChannelPriority,
}

impl Hash for EphemeralEnvSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self {
            dependencies,
            constraints,
            channels,
            exclude_newer,
            strategy,
            channel_priority,
        } = self;
        dependencies.hash(state);
        constraints.hash(state);
        channels.hash(state);
        exclude_newer.hash(state);
        // Neither SolveStrategy nor ChannelPriority implement Hash;
        // use the enum discriminant (both are C-like enums, so the
        // discriminant fully captures identity).
        mem::discriminant(strategy).hash(state);
        mem::discriminant(channel_priority).hash(state);
    }
}

impl PartialEq for EphemeralEnvSpec {
    fn eq(&self, other: &Self) -> bool {
        self.dependencies == other.dependencies
            && self.constraints == other.constraints
            && self.channels == other.channels
            && self.exclude_newer == other.exclude_newer
            && mem::discriminant(&self.strategy) == mem::discriminant(&other.strategy)
            && mem::discriminant(&self.channel_priority)
                == mem::discriminant(&other.channel_priority)
    }
}

impl Eq for EphemeralEnvSpec {}

impl EphemeralEnvSpec {
    /// Returns a stable cache key derived from the spec hash. The first
    /// segment is the alphabetically-first dependency package name (as
    /// a human-readable hint); the second is a URL-safe encoding of the
    /// spec hash.
    pub fn cache_key(&self) -> String {
        let hint: String = self
            .dependencies
            .iter_specs()
            .map(|(name, _)| name.as_normalized().to_string())
            .min()
            .unwrap_or_else(|| "env".to_string());
        let mut hasher = Xxh3::new();
        self.hash(&mut hasher);
        let encoded = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
        format!("{hint}-{encoded}")
    }
}

/// The key used to request an ephemeral env from the compute engine.
///
/// Wraps the spec in an [`Arc`] so dedup hits and subscribers clone
/// cheaply. Construct with [`EphemeralEnvKey::new`].
#[derive(Clone, Debug)]
pub struct EphemeralEnvKey(pub Arc<EphemeralEnvSpec>);

impl EphemeralEnvKey {
    pub fn new(spec: EphemeralEnvSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Hash for EphemeralEnvKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for EphemeralEnvKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Eq for EphemeralEnvKey {}

impl fmt::Display for EphemeralEnvKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.cache_key())
    }
}

/// The value returned by a successful [`EphemeralEnvKey`] compute.
#[derive(Debug)]
pub struct InstalledEphemeralEnv {
    /// The prefix where the environment was installed. Content-addressed.
    pub prefix: Prefix,

    /// The records that were installed. Useful for callers that need
    /// to look up a specific package version (e.g. the backend tool).
    pub records: Vec<PixiRecord>,
}

/// Errors that can be produced by [`EphemeralEnvKey::compute`].
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum EphemeralEnvError {
    #[error(
        "source dependencies are not supported in ephemeral environments (got source spec for `{}`)",
        .0.as_source()
    )]
    SourceSpecNotAllowed(PackageName),

    #[error("failed to convert a spec")]
    SpecConversion(#[source] Arc<pixi_spec::SpecConversionError>),

    #[error("failed to query the conda gateway")]
    Gateway(#[source] Arc<GatewayError>),

    #[error("failed to solve the environment")]
    Solve(#[source] Arc<SolveCondaEnvironmentError>),

    #[error("failed to construct the prefix at {0}")]
    CreatePrefix(PathBuf, #[source] Arc<std::io::Error>),

    #[error("failed to acquire the prefix lock at {0}")]
    AcquireLock(PathBuf, #[source] Arc<std::io::Error>),

    #[error("failed to update the prefix lock at {0}")]
    UpdateLock(PathBuf, #[source] Arc<std::io::Error>),

    #[error("failed to install the environment at {0}")]
    Install(PathBuf, #[source] Arc<InstallerError>),
}

impl Key for EphemeralEnvKey {
    type Value = Result<Arc<InstalledEphemeralEnv>, Arc<EphemeralEnvError>>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec: &EphemeralEnvSpec = &self.0;

        // Cross-process fast path: ephemeral env prefixes are
        // content-addressed by `spec.cache_key()`, so an existing
        // prefix at the derived path with a marker file recording
        // the install records is fully reusable. The compute-engine
        // cache only dedups within a single process, so without this
        // marker every `pixi run` re-fetches repodata and re-solves
        // the backend's binary deps — ~250 ms on Windows for
        // pixi-build-cmake / pixi-build-python — even when the
        // prefix on disk was already provisioned by a previous run.
        //
        let cache_key = spec.cache_key();
        let prefix_path = ctx.cache_dir::<BuildBackendsDir>().await.join(&cache_key);
        if let Some(cached) = read_cached_marker(prefix_path.as_std_path()).await {
            return Ok(Arc::new(cached));
        }

        // 1. Reject source specs, get a binary-only dep map.
        let binary_specs = match split_into_binary(&spec.dependencies) {
            Ok(b) => b,
            Err(name) => return Err(Arc::new(EphemeralEnvError::SourceSpecNotAllowed(name))),
        };

        // 2. Read the tool build environment (host + build + virtual packages).
        let build_env = ctx.compute(&ToolBuildEnvironmentKey).await;

        // 3. Fetch binary repodata.
        let binary_repodata = fetch_binary_repodata(ctx, spec, &binary_specs, &build_env)
            .await
            .map_err(Arc::new)?;

        // 4. Build a binary-only SolveCondaEnvironmentSpec and solve.
        let solve_spec = SolveCondaEnvironmentSpec {
            name: None,
            source_specs: DependencyMap::default(),
            binary_specs,
            constraints: spec.constraints.clone(),
            dev_source_records: Vec::new(),
            source_repodata: Vec::new(),
            binary_repodata,
            installed: Vec::new(),
            platform: build_env.host_platform,
            channels: spec.channels.clone(),
            virtual_packages: build_env.host_virtual_packages.clone(),
            strategy: spec.strategy,
            channel_priority: spec.channel_priority,
            exclude_newer: spec.exclude_newer.clone(),
        };
        let records = ctx
            .solve_conda(solve_spec)
            .await
            .map_err(|e| Arc::new(EphemeralEnvError::Solve(Arc::new(e))))?;

        // 5. Compute prefix path and create it. (`cache_key` and
        //    `prefix_path` were already derived above for the
        //    fast-path lookup.)
        let prefix_std = prefix_path.as_std_path().to_path_buf();
        let prefix = Prefix::create(prefix_std.clone()).map_err(|e| {
            Arc::new(EphemeralEnvError::CreatePrefix(
                prefix_std.clone(),
                Arc::new(e),
            ))
        })?;

        // 6. Cross-process lock + install + finish.
        let mut guard = AsyncPrefixGuard::new(prefix.path())
            .and_then(|g| g.write())
            .await
            .map_err(|e| {
                Arc::new(EphemeralEnvError::AcquireLock(
                    prefix.path().to_path_buf(),
                    Arc::new(e),
                ))
            })?;
        guard.begin().await.map_err(|e| {
            Arc::new(EphemeralEnvError::UpdateLock(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;

        // 7. Install the solved binaries.
        let binary_records = records
            .iter()
            .filter_map(|r| match r {
                PixiRecord::Binary(b) => Some(b.as_ref().clone()),
                PixiRecord::Source(_) => None,
            })
            .collect::<Vec<_>>();
        let data: &DataStore = ctx.global_data();
        let install_reporter = data
            .instantiate_backend_reporter()
            .and_then(|r| InstantiateBackendReporter::create_install_reporter(r.as_ref()));
        install_binary_records(
            data,
            &prefix,
            binary_records,
            build_env.host_platform,
            install_reporter,
        )
        .await
        .map_err(|e| {
            Arc::new(EphemeralEnvError::Install(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;

        guard.finish().await.map_err(|e| {
            Arc::new(EphemeralEnvError::UpdateLock(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;

        // Persist a marker so the next process can hit the
        // fast-path without re-fetching repodata or re-solving.
        // Best-effort: a write failure just costs the next caller
        // one solve.
        let binary_records: Vec<RepoDataRecord> = records
            .iter()
            .filter_map(|r| match r {
                PixiRecord::Binary(b) => Some(b.as_ref().clone()),
                PixiRecord::Source(_) => None,
            })
            .collect();
        write_cached_marker(prefix.path(), &binary_records).await;

        Ok(Arc::new(InstalledEphemeralEnv { prefix, records }))
    }
}

/// Marker filename written next to the ephemeral-env prefix's
/// `conda-meta` directory. Encodes the records the prefix was
/// provisioned with so a subsequent process can short-circuit the
/// repodata fetch + solve when the prefix is reused.
const CACHE_MARKER_FILENAME: &str = ".pixi-ephemeral-cache.json";

/// Schema written to [`CACHE_MARKER_FILENAME`].
///
/// `version` lets us reject markers from an older / incompatible
/// pixi without parsing their payload, so an in-place upgrade can
/// invalidate the cache cleanly.
#[derive(Serialize, Deserialize)]
struct EphemeralEnvCacheMarker {
    version: u32,
    records: Vec<RepoDataRecord>,
}

const EPHEMERAL_ENV_CACHE_MARKER_VERSION: u32 = 1;

/// Read and validate the per-prefix cache marker.
///
/// Returns `Some(InstalledEphemeralEnv)` only when the prefix
/// directory exists, the marker parses, and the recorded
/// `version` matches what this build of pixi understands. Any
/// other failure (missing prefix / marker / parse error / version
/// mismatch) returns `None` so the caller falls through to the
/// full solve+install path.
async fn read_cached_marker(prefix_path: &std::path::Path) -> Option<InstalledEphemeralEnv> {
    let marker_path = prefix_path.join(CACHE_MARKER_FILENAME);
    let bytes = tokio::fs::read(&marker_path).await.ok()?;
    let marker: EphemeralEnvCacheMarker = serde_json::from_slice(&bytes).ok()?;
    if marker.version != EPHEMERAL_ENV_CACHE_MARKER_VERSION {
        return None;
    }
    // The prefix needs to actually exist; if it has been wiped
    // (e.g. `pixi clean`) but the marker survived, we must
    // re-install.
    let prefix = Prefix::create(prefix_path.to_path_buf()).ok()?;
    let records = marker
        .records
        .into_iter()
        .map(|r| PixiRecord::Binary(Arc::new(r)))
        .collect();
    Some(InstalledEphemeralEnv { prefix, records })
}

/// Write the per-prefix cache marker atomically (write-temp +
/// rename). Failures are swallowed: skipping the write only costs
/// the next caller one extra solve, never correctness.
async fn write_cached_marker(prefix_path: &std::path::Path, records: &[RepoDataRecord]) {
    let marker = EphemeralEnvCacheMarker {
        version: EPHEMERAL_ENV_CACHE_MARKER_VERSION,
        records: records.to_vec(),
    };
    let Ok(bytes) = serde_json::to_vec(&marker) else {
        return;
    };
    let dest = prefix_path.join(CACHE_MARKER_FILENAME);
    let tmp = prefix_path.join(format!("{CACHE_MARKER_FILENAME}.tmp"));
    if tokio::fs::write(&tmp, &bytes).await.is_err() {
        return;
    }
    let _ = tokio::fs::rename(&tmp, &dest).await;
}

/// Fetch binary repodata for the spec's dependencies + constraints.
async fn fetch_binary_repodata(
    ctx: &mut ComputeCtx,
    spec: &EphemeralEnvSpec,
    binary_specs: &DependencyMap<PackageName, BinarySpec>,
    build_env: &crate::BuildEnvironment,
) -> Result<Vec<RepoData>, EphemeralEnvError> {
    use rattler_conda_types::{Channel, Platform};

    let channel_config = ctx.compute(&ChannelConfigKey).await;
    let gateway = ctx.global_data().gateway().clone();
    let gateway_reporter = ctx
        .global_data()
        .conda_solve_reporter()
        .and_then(|r| r.create_gateway_reporter());

    let match_specs = binary_specs
        .clone()
        .into_match_specs(&channel_config)
        .map_err(|e| EphemeralEnvError::SpecConversion(Arc::new(e)))?;
    let constraint_specs = spec
        .constraints
        .clone()
        .into_match_specs(&channel_config)
        .map_err(|e| EphemeralEnvError::SpecConversion(Arc::new(e)))?;

    let mut query = gateway
        .query(
            spec.channels.iter().cloned().map(Channel::from_url),
            [build_env.host_platform, Platform::NoArch],
            match_specs.into_iter().chain(constraint_specs),
        )
        .recursive(true);
    if let Some(reporter) = gateway_reporter {
        query = query.with_reporter(WrappingGatewayReporter(reporter));
    }
    query
        .await
        .map_err(|e| EphemeralEnvError::Gateway(Arc::new(e)))
}

/// Validate that every dependency resolves to a [`BinarySpec`] and
/// return them as a new [`DependencyMap`]. Returns the first offending
/// package name on source.
fn split_into_binary(
    deps: &DependencyMap<PackageName, PixiSpec>,
) -> Result<DependencyMap<PackageName, BinarySpec>, PackageName> {
    let mut out: BTreeMap<PackageName, Vec<BinarySpec>> = BTreeMap::new();
    for (name, spec) in deps.iter_specs() {
        match spec.clone().into_source_or_binary() {
            Either::Right(bin) => {
                out.entry(name.clone()).or_default().push(bin);
            }
            Either::Left(_) => return Err(name.clone()),
        }
    }
    let mut result = DependencyMap::<PackageName, BinarySpec>::default();
    for (name, specs) in out {
        for spec in specs {
            result.insert(name.clone(), spec);
        }
    }
    Ok(result)
}
