//! Content-addressed compute-engine Key for disposable, binary-only conda
//! environments. Source specs are rejected up-front.
//!
//! The on-disk cache is two-level: prefixes are addressed by a
//! fingerprint of the *resolved* records, and a spec-keyed pointer file
//! ([`EphemeralEnvPointer`]) maps a spec to the fingerprint it last
//! resolved to so a later process can skip the solve.
//!
//! Prefixes are locked with [`EnvironmentLock`] for cross-process safety.
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
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use itertools::Either;
use miette::Diagnostic;
use pixi_compute_engine::{ComputeCtx, DataStore, Key};
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, PixiSpec, ResolvedExcludeNewer};
use pixi_spec_containers::DependencyMap;
use pixi_utils::{EnvironmentFingerprint, EnvironmentLock};
use rattler::install::InstallerError;
use rattler_conda_types::{ChannelUrl, PackageName, RepoDataRecord, prefix::Prefix};
use rattler_repodata_gateway::{GatewayError, RepoData};
use rattler_solve::{ChannelPriority, SolveStrategy};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

use crate::SolveCondaEnvironmentSpec;
use crate::cache::markers::BuildBackendsDir;
use crate::compute_data::{HasGateway, HasGatewayReporter, HasInstantiateBackendReporter};
use crate::injected_config::{ChannelConfigKey, ToolBuildEnvironmentKey};
use crate::install_binary::install_binary_records;
use crate::reporter::{InstantiateBackendReporter, WrappingGatewayReporter};
use crate::solve_binary::SolveCondaExt;
use crate::solve_conda::SolveCondaEnvironmentError;
use pixi_compute_cache_dirs::CacheDirsExt;
use pixi_compute_reporters::OperationId;

/// How often to warn while blocked on a peer's install lock.
const EPHEMERAL_LOCK_PROGRESS_INTERVAL: Duration = Duration::from_secs(30);

/// Specification for an ephemeral, binary-only conda environment.
///
/// The spec's hash keys the compute-engine dedup; the on-disk prefix is
/// content-addressed on the *resolved* records (see `prefix_cache_key`).
/// Source `PixiSpec`s in `dependencies` cause the Key to fail at compute
/// time.
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
        self.hash_without_exclude_newer(state);
        self.exclude_newer.hash(state);
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
    /// Hash every field except `exclude_newer`, so the pointer file name
    /// can be keyed on the cutoff-independent spec identity.
    fn hash_without_exclude_newer<H: Hasher>(&self, state: &mut H) {
        let Self {
            dependencies,
            constraints,
            channels,
            exclude_newer: _,
            strategy,
            channel_priority,
        } = self;
        dependencies.hash(state);
        constraints.hash(state);
        channels.hash(state);
        // Neither SolveStrategy nor ChannelPriority implement Hash;
        // use the enum discriminant (both are C-like enums, so the
        // discriminant fully captures identity).
        mem::discriminant(strategy).hash(state);
        mem::discriminant(channel_priority).hash(state);
    }

    /// Human-readable hint for the ephemeral env: the alphabetically-first
    /// dependency package name, or `env` when there are no dependencies.
    fn dependency_hint(&self) -> String {
        self.dependencies
            .iter_specs()
            .map(|(name, _)| name.as_normalized().to_string())
            .min()
            .unwrap_or_else(|| "env".to_string())
    }

    /// Content-addressed cache key for the *resolved* environment:
    /// `<hint>-<fingerprint of the resolved records>`. Keying on the
    /// solve's output keeps the prefix path stable across a moving
    /// (relative) `exclude-newer` cutoff.
    fn prefix_cache_key(&self, fingerprint: &EnvironmentFingerprint) -> String {
        format!("{}-{}", self.dependency_hint(), fingerprint.as_str())
    }

    /// File name of this spec's [`EphemeralEnvPointer`]:
    /// `<hint>-<hash of the spec sans cutoff>.pointer.json`. The cutoff is
    /// stored *inside* the pointer rather than in the name so a moving
    /// cutoff overwrites one file per spec instead of accreting one per
    /// resolved cutoff.
    fn pointer_file_name(&self) -> String {
        let mut hasher = Xxh3::new();
        self.hash_without_exclude_newer(&mut hasher);
        let encoded = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
        format!("{}-{}.pointer.json", self.dependency_hint(), encoded)
    }

    /// Stable per-spec label, for display purposes only.
    fn label(&self) -> String {
        let mut hasher = Xxh3::new();
        self.hash(&mut hasher);
        let encoded = URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes());
        format!("{}-{}", self.dependency_hint(), encoded)
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
        write!(f, "{}", self.0.label())
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

        // Cross-process fast path: if the pointer matches the spec's
        // cutoff and the prefix it points at carries a marker, the env is
        // reusable without a repodata fetch or solve. A relative
        // `exclude-newer` yields a fresh cutoff every invocation and never
        // matches here.
        let backends_dir = ctx.cache_dir::<BuildBackendsDir>().await;
        let pointer_path = backends_dir.join(spec.pointer_file_name());
        if let Some(pointer) = read_pointer(pointer_path.as_std_path()).await
            && pointer.exclude_newer == spec.exclude_newer
        {
            let prefix_path = backends_dir.join(spec.prefix_cache_key(&pointer.fingerprint));
            if let Some(cached) = read_cached_marker(prefix_path.as_std_path()).await {
                return Ok(Arc::new(cached));
            }
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

        // 5. Content-address the prefix on the *resolved* records: solves
        //    that resolve to the same packages share one prefix even when
        //    a relative `exclude-newer` cutoff moves between invocations.
        let binary_records = records
            .iter()
            .filter_map(|r| match r {
                PixiRecord::Binary(b) => Some(b.as_ref().clone()),
                PixiRecord::Source(_) => None,
            })
            .collect::<Vec<_>>();
        let expected_fingerprint = EnvironmentFingerprint::compute(binary_records.iter());

        let cache_key = spec.prefix_cache_key(&expected_fingerprint);
        let prefix_path = backends_dir.join(&cache_key);

        // A prefix already provisioned for these records is reusable;
        // refresh the pointer so the next process can skip the solve.
        if let Some(cached) = read_cached_marker(prefix_path.as_std_path()).await {
            write_pointer(pointer_path.as_std_path(), spec, &expected_fingerprint).await;
            return Ok(Arc::new(cached));
        }

        // Create the prefix directory.
        let prefix_std = prefix_path.as_std_path().to_path_buf();
        let prefix = Prefix::create(prefix_std.clone()).map_err(|e| {
            Arc::new(EphemeralEnvError::CreatePrefix(
                prefix_std.clone(),
                Arc::new(e),
            ))
        })?;

        // 6. Cross-process install lock + recheck + install.
        let prefix_display = prefix.path().display().to_string();
        let mut env_lock = EnvironmentLock::acquire_with_progress(
            prefix.path(),
            EPHEMERAL_LOCK_PROGRESS_INTERVAL,
            |elapsed| {
                tracing::warn!(
                    "still waiting on another pixi process to finish installing '{prefix_display}' ({}s elapsed)",
                    elapsed.as_secs(),
                );
            },
        )
        .await
        .map_err(|e| {
            Arc::new(EphemeralEnvError::AcquireLock(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;
        // Recheck under the lock for a peer that just finished.
        if env_lock.matches(&expected_fingerprint)
            && let Some(cached) = read_cached_marker(prefix.path()).await
        {
            write_pointer(pointer_path.as_std_path(), spec, &expected_fingerprint).await;
            return Ok(Arc::new(cached));
        }

        // A previous install here crashed; re-link everything.
        let reinstall_all = env_lock.was_interrupted();
        env_lock.begin().await.map_err(|e| {
            Arc::new(EphemeralEnvError::UpdateLock(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;

        // 7. Install the solved binaries.
        let data: &DataStore = ctx.global_data();
        let install_reporter = data
            .instantiate_backend_reporter()
            .and_then(|r| InstantiateBackendReporter::create_install_reporter(r.as_ref()));
        install_binary_records(
            data,
            &prefix,
            binary_records.clone(),
            build_env.host_platform,
            reinstall_all,
            install_reporter,
        )
        .await
        .map_err(|e| {
            Arc::new(EphemeralEnvError::Install(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;

        // Write the records marker before releasing the lock so peers
        // see it on the lock-free fast path, then refresh the pointer.
        write_cached_marker(prefix.path(), &binary_records).await;
        write_pointer(pointer_path.as_std_path(), spec, &expected_fingerprint).await;

        env_lock.finish(&expected_fingerprint).await.map_err(|e| {
            Arc::new(EphemeralEnvError::UpdateLock(
                prefix.path().to_path_buf(),
                Arc::new(e),
            ))
        })?;

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

/// Spec → fingerprint pointer, stored at
/// [`EphemeralEnvSpec::pointer_file_name`] in the backends cache dir.
///
/// Purely advisory: a missing, stale, or corrupt pointer costs one
/// solve, never correctness. The stored `exclude_newer` must equal the
/// spec's before the pointer is followed.
#[derive(Serialize, Deserialize)]
struct EphemeralEnvPointer {
    version: u32,
    exclude_newer: Option<ResolvedExcludeNewer>,
    fingerprint: EnvironmentFingerprint,
}

const EPHEMERAL_ENV_POINTER_VERSION: u32 = 1;

/// Read and validate a spec → fingerprint pointer. Returns `None` on a
/// missing, unparsable, or wrong-version pointer, or when the
/// fingerprint is not plain hex (it becomes a path segment).
async fn read_pointer(pointer_path: &std::path::Path) -> Option<EphemeralEnvPointer> {
    let bytes = tokio::fs::read(pointer_path).await.ok()?;
    let pointer: EphemeralEnvPointer = serde_json::from_slice(&bytes).ok()?;
    if pointer.version != EPHEMERAL_ENV_POINTER_VERSION {
        return None;
    }
    let fingerprint = pointer.fingerprint.as_str();
    if fingerprint.is_empty() || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(pointer)
}

/// Write the spec → fingerprint pointer atomically. Failures are
/// swallowed: skipping the write only costs the next caller one extra
/// solve, never correctness.
async fn write_pointer(
    pointer_path: &std::path::Path,
    spec: &EphemeralEnvSpec,
    fingerprint: &EnvironmentFingerprint,
) {
    let pointer = EphemeralEnvPointer {
        version: EPHEMERAL_ENV_POINTER_VERSION,
        exclude_newer: spec.exclude_newer.clone(),
        fingerprint: fingerprint.clone(),
    };
    let Ok(bytes) = serde_json::to_vec(&pointer) else {
        return;
    };
    let _ = pixi_utils::atomic_write::atomic_write(pointer_path, &bytes).await;
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
    // `fetch_binary_repodata` is invoked from `EphemeralEnvKey::compute`,
    // which runs inside the backend-instantiate op's `scope_active`.
    let gateway_reporter = OperationId::current().and_then(|op_id| {
        ctx.global_data()
            .gateway_reporter()
            .and_then(|r| r.create_gateway_reporter(op_id))
    });

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
        .map(|output| output.repodata)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn spec_with_exclude_newer(exclude_newer: Option<ResolvedExcludeNewer>) -> EphemeralEnvSpec {
        EphemeralEnvSpec {
            dependencies: DependencyMap::default(),
            constraints: DependencyMap::default(),
            channels: Vec::new(),
            exclude_newer,
            strategy: SolveStrategy::default(),
            channel_priority: ChannelPriority::default(),
        }
    }

    fn resolved(cutoff: &str) -> ResolvedExcludeNewer {
        ResolvedExcludeNewer::from_datetime(cutoff.parse::<DateTime<Utc>>().unwrap())
    }

    /// Solves that resolve to the same records share one prefix
    /// regardless of the `exclude-newer` cutoff.
    #[test]
    fn prefix_cache_key_ignores_exclude_newer() {
        let fingerprint = EnvironmentFingerprint::from_string("0123456789abcdef".to_string());
        let none = spec_with_exclude_newer(None);
        let a = spec_with_exclude_newer(Some(resolved("2026-01-01T00:00:00Z")));
        let b = spec_with_exclude_newer(Some(resolved("2026-07-16T09:00:00Z")));

        let key = none.prefix_cache_key(&fingerprint);
        assert_eq!(key, a.prefix_cache_key(&fingerprint));
        assert_eq!(key, b.prefix_cache_key(&fingerprint));
    }

    /// Different resolved records yield different prefixes.
    #[test]
    fn prefix_cache_key_changes_with_resolved_records() {
        let spec = spec_with_exclude_newer(Some(resolved("2026-01-01T00:00:00Z")));
        let fp1 = EnvironmentFingerprint::from_string("0123456789abcdef".to_string());
        let fp2 = EnvironmentFingerprint::from_string("fedcba9876543210".to_string());
        assert_ne!(spec.prefix_cache_key(&fp1), spec.prefix_cache_key(&fp2));
    }

    /// The compute-engine key still distinguishes specs that differ only
    /// in `exclude_newer`, so different cutoffs are solved independently.
    #[test]
    fn spec_identity_still_distinguishes_exclude_newer() {
        let a = spec_with_exclude_newer(Some(resolved("2024-01-01T00:00:00Z")));
        let b = spec_with_exclude_newer(Some(resolved("2026-01-01T00:00:00Z")));
        assert_ne!(a, b);
        assert_ne!(a.label(), b.label());
    }

    /// The pointer round-trips through disk and carries the cutoff it
    /// was written under.
    #[tokio::test]
    async fn pointer_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let spec = spec_with_exclude_newer(Some(resolved("2026-01-01T00:00:00Z")));
        let path = dir.path().join(spec.pointer_file_name());
        let fingerprint = EnvironmentFingerprint::from_string("0123456789abcdef".to_string());

        assert!(read_pointer(&path).await.is_none());
        write_pointer(&path, &spec, &fingerprint).await;
        let pointer = read_pointer(&path).await.unwrap();
        assert_eq!(pointer.fingerprint, fingerprint);
        assert_eq!(pointer.exclude_newer, spec.exclude_newer);
    }

    /// Specs that differ only in their cutoff share one pointer file, but
    /// the stored cutoff keeps them distinct.
    #[tokio::test]
    async fn pointer_is_shared_per_spec_but_keyed_on_cutoff() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = spec_with_exclude_newer(Some(resolved("2026-01-01T00:00:00Z")));
        let b = spec_with_exclude_newer(Some(resolved("2026-07-16T09:00:00Z")));
        assert_eq!(a.pointer_file_name(), b.pointer_file_name());

        let path = dir.path().join(a.pointer_file_name());
        let fingerprint = EnvironmentFingerprint::from_string("0123456789abcdef".to_string());
        write_pointer(&path, &a, &fingerprint).await;

        let pointer = read_pointer(&path).await.unwrap();
        assert_eq!(pointer.exclude_newer, a.exclude_newer);
        assert_ne!(pointer.exclude_newer, b.exclude_newer);
    }

    /// Corrupt or incompatible pointers are rejected.
    #[tokio::test]
    async fn read_pointer_rejects_invalid_contents() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("env-abc.pointer.json");

        tokio::fs::write(&path, b"not json").await.unwrap();
        assert!(read_pointer(&path).await.is_none());

        tokio::fs::write(
            &path,
            br#"{"version":999,"exclude_newer":null,"fingerprint":"0123456789abcdef"}"#,
        )
        .await
        .unwrap();
        assert!(read_pointer(&path).await.is_none());

        tokio::fs::write(
            &path,
            br#"{"version":1,"exclude_newer":null,"fingerprint":"../escape"}"#,
        )
        .await
        .unwrap();
        assert!(read_pointer(&path).await.is_none());
    }
}
