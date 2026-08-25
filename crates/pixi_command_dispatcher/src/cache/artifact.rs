//! Artifact cache for source builds.
//!
//! Layout (under the workspace's `.pixi/artifacts-v0/`, or the global
//! cache root when no workspace is set):
//!
//! ```text
//! <package_name>/<cache_key>/
//!     <pkg>-<ver>-<build>.conda
//!     sidecar.json
//! ```
//!
//! The cache key is structural (identity of the build inputs plus content
//! addresses of its dependencies). For mutable sources, freshness of the
//! source files themselves is tracked in the sidecar via per-file
//! fingerprints, plus a re-glob pass that detects added files. The mtime
//! remains the fast path; contents are hashed only when it changes.
//! Immutable sources are fully pinned by the key, so their entries carry no
//! file lists and skip those checks.
//!
//! On a hit, the cached `.conda` is returned along with its sha256; no
//! backend work happens. On a miss (or stale sidecar), the caller rebuilds
//! and calls [`ArtifactCache::store`] to populate the entry.
//!
//! Failed builds are never persisted; the compute engine caches failures in
//! memory for the lifetime of the process.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use async_fd_lock::{LockRead, LockWrite};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use pixi_build_types::InputGlobSet;
use pixi_build_discovery::{
    CommandSpec, EnvironmentSpec, JsonRpcBackendSpec, SystemCommandSpec,
};
use pixi_compute_engine::ComputeCtx;
use pixi_manifest::InlineContentHash;
use pixi_path::{AbsPath, AbsPathBuf};
use pixi_record::{UnresolvedPixiRecord, UnresolvedSourceRecord};
use pixi_spec::{PixiSpec, ResolvedExcludeNewer};
use rattler_conda_types::{ChannelUrl, PackageName, Platform, RepoDataRecord};
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use tokio::sync::Semaphore;
use url::Url;
use xxhash_rust::xxh3::Xxh3;

use crate::file_fingerprint::spawn_blocking_with_io_permit;
use crate::input_snapshot::{
    InputFileState, InputSnapshot, SnapshotFreshness, StaleFile, StaleFileReason,
};

/// Whether the source files behind a cache entry can change without the
/// pinned source spec changing.
///
/// Derived from the pinned manifest and build sources: only path pins are
/// mutable, git commits and url archives are immutable. For an immutable
/// source the cache key fully determines the artifact content, so lookup
/// skips the per-file freshness checks entirely. This keeps entries valid
/// when the recorded input files (which live in the global cache dir for
/// git sources) have been deleted, e.g. on a CI runner that only restored
/// the workspace's `.pixi` directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMutability {
    Mutable,
    Immutable,
}

/// Opaque content-addressed handle for an artifact cache entry.
///
/// Format: url-safe-base64 of the `xxh3_64` over all hashed inputs
/// (see [`compute_artifact_cache_key`]). Package name is not included
/// because the entry lives under a `<package_name>/` parent directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactCacheKey(String);

/// Cache key for locating an artifact produced from immutable Git sources
/// before its build backend has been resolved.
///
/// This is deliberately distinct from [`ArtifactCacheKey`]: it omits the
/// backend identifier, which is only available after source checkout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImmutableArtifactCacheKey(String);

impl std::fmt::Display for ImmutableArtifactCacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The checkout-independent subset of a backend specification that can be
/// restored from an immutable artifact alias. Constraint-bearing environment
/// specs are intentionally excluded: [`pixi_spec::BinarySpec`] is not a
/// deserializable cache format, and skipping those aliases is safer than
/// weakening their backend resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum ImmutableBackendSpec {
    Environment {
        name: String,
        requirement: (PackageName, PixiSpec),
        additional_requirements: Vec<(PackageName, PixiSpec)>,
        channels: Vec<ChannelUrl>,
        command: Option<String>,
    },
    System {
        name: String,
        command: Option<String>,
    },
}

impl ImmutableBackendSpec {
    fn from_backend_spec(spec: JsonRpcBackendSpec) -> Option<Self> {
        match spec.command {
            CommandSpec::EnvironmentSpec(env_spec) if env_spec.constraints.is_empty() => {
                Some(Self::Environment {
                    name: spec.name,
                    requirement: env_spec.requirement,
                    additional_requirements: env_spec.additional_requirements.into_specs().collect(),
                    channels: env_spec.channels,
                    command: env_spec.command,
                })
            }
            CommandSpec::System(system_spec) => Some(Self::System {
                name: spec.name,
                command: system_spec.command,
            }),
            CommandSpec::EnvironmentSpec(_) => None,
        }
    }

    pub(crate) fn into_backend_spec(self) -> JsonRpcBackendSpec {
        match self {
            Self::Environment {
                name,
                requirement,
                additional_requirements,
                channels,
                command,
            } => JsonRpcBackendSpec {
                name,
                command: CommandSpec::EnvironmentSpec(Box::new(EnvironmentSpec {
                    requirement,
                    additional_requirements: additional_requirements.into_iter().collect(),
                    constraints: Default::default(),
                    channels,
                    command,
                })),
            },
            Self::System { name, command } => JsonRpcBackendSpec {
                name,
                command: CommandSpec::System(SystemCommandSpec { command }),
            },
        }
    }
}

/// An immutable-source lookup entry pointing at the normal artifact-cache
/// entry. The regular artifact entry remains authoritative.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ImmutableArtifactAlias {
    artifact_key: ArtifactCacheKey,
    #[serde_as(as = "rattler_digest::serde::SerializableHash<rattler_digest::Sha256>")]
    artifact_sha256: Sha256Hash,
    pub backend_identifier: String,
    pub backend_spec: ImmutableBackendSpec,
}

impl std::fmt::Display for ArtifactCacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Compute the checkout-free lookup key for an immutable source build.
///
/// This has the same structural inputs as [`compute_artifact_cache_key`],
/// except for the backend identifier. An alias written after a normal build
/// connects this key to the backend-specific artifact entry.
#[allow(clippy::too_many_arguments)]
pub fn compute_immutable_artifact_cache_key(
    record: &UnresolvedSourceRecord,
    build_platform: Platform,
    host_platform: Platform,
    channels: &[ChannelUrl],
    exclude_newer: &Option<ResolvedExcludeNewer>,
    build_source_dep_sha256s: &[Sha256Hash],
    host_source_dep_sha256s: &[Sha256Hash],
    project_model_overrides: &crate::ProjectModelOverrides,
    package_format: Option<pixi_build_types::procedures::conda_build_v1::CondaPackageFormat>,
    inline_content_hash: Option<InlineContentHash>,
) -> ImmutableArtifactCacheKey {
    let mut hasher = Xxh3::new();
    "immutable-artifact-alias-v1".hash(&mut hasher);
    record.name().as_normalized().hash(&mut hasher);
    record.manifest_source.hash(&mut hasher);
    record.build_source.hash(&mut hasher);
    record.variants.hash(&mut hasher);
    inline_content_hash.hash(&mut hasher);
    build_platform.hash(&mut hasher);
    host_platform.hash(&mut hasher);
    channels.hash(&mut hasher);
    exclude_newer.hash(&mut hasher);
    project_model_overrides.hash(&mut hasher);
    package_format.hash(&mut hasher);

    "build_packages".hash(&mut hasher);
    for dep in &record.build_packages {
        if let UnresolvedPixiRecord::Binary(repo) = dep {
            repo.url.as_str().hash(&mut hasher);
            repo.package_record.sha256.hash(&mut hasher);
        }
    }
    for sha in build_source_dep_sha256s {
        sha.hash(&mut hasher);
    }

    "host_packages".hash(&mut hasher);
    for dep in &record.host_packages {
        if let UnresolvedPixiRecord::Binary(repo) = dep {
            repo.url.as_str().hash(&mut hasher);
            repo.package_record.sha256.hash(&mut hasher);
        }
    }
    for sha in host_source_dep_sha256s {
        sha.hash(&mut hasher);
    }

    ImmutableArtifactCacheKey(URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes()))
}

/// Compute the artifact cache key for a source build.
///
/// Source *files* are deliberately omitted: mutable sources are validated by
/// the sidecar's mtimes and globs, while immutable sources use their locked
/// commit identity through [`compute_immutable_artifact_cache_key`].
#[allow(clippy::too_many_arguments)]
pub fn compute_artifact_cache_key(
    record: &UnresolvedSourceRecord,
    build_platform: Platform,
    host_platform: Platform,
    backend_identifier: &str,
    build_source_dep_sha256s: &[Sha256Hash],
    host_source_dep_sha256s: &[Sha256Hash],
    project_model_overrides: &crate::ProjectModelOverrides,
    package_format: Option<pixi_build_types::procedures::conda_build_v1::CondaPackageFormat>,
    inline_content_hash: Option<InlineContentHash>,
) -> ArtifactCacheKey {
    let mut hasher = Xxh3::new();
    record.name().as_normalized().hash(&mut hasher);
    record.manifest_source.hash(&mut hasher);
    record.build_source.hash(&mut hasher);
    record.variants.hash(&mut hasher);
    // An inline package definition's content hash is not otherwise
    // represented on disk, so it must enter the key explicitly: editing the
    // inline `[package]` table then invalidates the built artifact even when the
    // source files are untouched. `None` for ordinary source packages keeps
    // their key unchanged.
    inline_content_hash.hash(&mut hasher);
    build_platform.hash(&mut hasher);
    host_platform.hash(&mut hasher);
    backend_identifier.hash(&mut hasher);
    project_model_overrides.hash(&mut hasher);
    // Distinguish artifacts by output format.
    package_format.hash(&mut hasher);

    // Bucket-tagged streams: the same (url, sha256) behaves differently
    // when installed into the build prefix vs. the host prefix because
    // run-dep resolution uses different pin-compatibility maps. Hash a
    // distinct marker per bucket so the two cases can never collide.
    "build_packages".hash(&mut hasher);
    for dep in &record.build_packages {
        if let UnresolvedPixiRecord::Binary(repo) = dep {
            repo.url.as_str().hash(&mut hasher);
            repo.package_record.sha256.hash(&mut hasher);
        }
    }
    for sha in build_source_dep_sha256s {
        sha.hash(&mut hasher);
    }

    "host_packages".hash(&mut hasher);
    for dep in &record.host_packages {
        if let UnresolvedPixiRecord::Binary(repo) = dep {
            repo.url.as_str().hash(&mut hasher);
            repo.package_record.sha256.hash(&mut hasher);
        }
    }
    for sha in host_source_dep_sha256s {
        sha.hash(&mut hasher);
    }

    // `host_platform` is already folded into `hasher`, so the hash
    // alone uniquely identifies the artifact. Dropping the display-only
    // prefix keeps the on-disk path short on Windows.
    ArtifactCacheKey(URL_SAFE_NO_PAD.encode(hasher.finish().to_ne_bytes()))
}

/// On-disk record that lives next to a cached `.conda`.
///
/// Format has no version field by design: a parse failure triggers cache
/// invalidation, and fields are only ever added in a serde-backwards-compatible
/// way (new fields get `#[serde(default)]`).
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSidecar {
    /// Structured glob groups describing the files the build reads (the flat
    /// globs a backend reports are folded into a group upstream). Walked at
    /// lookup time to detect newly-added matching files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_glob_sets: Vec<InputGlobSet>,

    /// Files that matched the input globs, paired with their mtime at build
    /// time. Keyed by absolute path (the walk roots are absolute), so the keys
    /// round-trip directly against the engine walk at lookup time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_files: BTreeMap<AbsPathBuf, DateTime<Utc>>,

    /// Validation state for the input files. Sidecars written by older pixi
    /// versions omit this map; those files keep mtime-only validation via
    /// `input_files`.
    #[serde(default, skip_serializing_if = "InputSnapshot::is_empty")]
    pub(crate) input_file_states: InputSnapshot,

    /// sha256 of the cached `.conda`. Callers rely on this for transitive
    /// invalidation of dependents.
    #[serde_as(as = "rattler_digest::serde::SerializableHash<rattler_digest::Sha256>")]
    pub artifact_sha256: Sha256Hash,

    /// File name of the cached `.conda` within the entry directory.
    pub artifact_filename: String,

    /// Fully-assembled `RepoDataRecord` for the artifact. Stored so cache
    /// hits can return it without re-reading index.json from the `.conda`.
    pub record: RepoDataRecord,
}

impl ArtifactSidecar {
    /// Builds the record for a freshly stored artifact. `input_files` is
    /// derived from the snapshot; it stays on the wire so older pixi
    /// versions can keep validating the entry.
    fn new(
        input_glob_sets: Vec<InputGlobSet>,
        snapshot: InputSnapshot,
        artifact_sha256: Sha256Hash,
        artifact_filename: String,
        record: RepoDataRecord,
    ) -> Self {
        let input_files = snapshot
            .iter()
            .map(|(path, state)| (path.clone(), DateTime::<Utc>::from(state.modified())))
            .collect();
        Self {
            input_glob_sets,
            input_files,
            input_file_states: snapshot,
            artifact_sha256,
            artifact_filename,
            record,
        }
    }

    /// The snapshot to verify against. Sidecars written before fingerprints
    /// existed carry only `input_files` mtimes, which become mtime-only
    /// states; everything newer verifies its recorded states directly.
    fn input_snapshot(&self) -> Cow<'_, InputSnapshot> {
        if !self.input_file_states.is_empty() {
            return Cow::Borrowed(&self.input_file_states);
        }
        let mut snapshot = InputSnapshot::default();
        for (path, mtime) in &self.input_files {
            snapshot.insert_fallback(
                path.clone(),
                InputFileState::MtimeOnly(SystemTime::from(*mtime)),
            );
        }
        Cow::Owned(snapshot)
    }
}

/// Result of a successful cache lookup.
#[derive(Debug, Clone)]
pub struct CachedArtifact {
    pub artifact: PathBuf,
    pub sha256: Sha256Hash,
    pub record: RepoDataRecord,
}

/// Outcome of [`ArtifactCache::try_refresh_sidecar`].
enum SidecarRefresh {
    /// The refreshed sidecar was persisted over the validated bytes.
    Written,
    /// Another process changed the entry in the meantime; nothing was
    /// written.
    Changed,
}

/// Outcome of an [`ArtifactCache::lookup`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum CacheLookup {
    /// A usable entry was found; no build work is needed.
    Hit(CachedArtifact),

    /// No usable entry. The caller has to rebuild, and the reason explains
    /// why the existing state could not be reused.
    Miss(CacheMissReason),
}

/// Why a lookup did not yield a usable entry.
///
/// Carried out of the cache so a rebuild can explain itself: a changed cache
/// key and an invalidated entry both end in "rebuild", but they point at very
/// different causes (a dependency or backend moved vs. a source file was
/// touched), and only the cache can tell them apart.
#[derive(Debug, Clone)]
pub enum CacheMissReason {
    /// Nothing is stored under this cache key. Either the package was never
    /// built here, or one of the key's inputs changed (dependencies,
    /// variants, platforms, backend version, package format) and the build
    /// moved to a fresh key, leaving the previous entry behind untouched.
    NoEntry,

    /// The sidecar exists but could not be parsed, so nothing it claims about
    /// the entry can be trusted.
    CorruptSidecar,

    /// A file that was recorded as a build input no longer exists.
    InputFileRemoved { path: AbsPathBuf },

    /// A recorded input file exists but could not be read, so its contents
    /// cannot be verified.
    InputFileUnreadable { path: AbsPathBuf },

    /// The entry records no validation state for a file it lists as an
    /// input, so nothing can verify it.
    InputFileUntracked { path: AbsPathBuf },

    /// A recorded input file's mtime no longer matches the one captured when
    /// the artifact was built.
    InputFileModified {
        path: AbsPathBuf,
        built_at: DateTime<Utc>,
        modified_at: DateTime<Utc>,
    },

    /// A file matching the recorded input globs was not present when the
    /// artifact was built.
    InputFileAdded { path: AbsPathBuf },

    /// The sidecar is valid but the `.conda` it points at is gone.
    ArtifactRemoved { path: PathBuf },

    /// A recorded input file was modified while the entry was being built,
    /// so its recorded hash cannot vouch for what the build read. One
    /// rebuild confirms it.
    InputFileUnconfirmed { path: AbsPathBuf },

    /// The entry changed while it was being validated: a concurrent store
    /// replaced it or a concurrent clear removed it.
    EntryChanged,
}

impl std::fmt::Display for CacheMissReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEntry => write!(f, "no entry stored for this cache key"),
            Self::CorruptSidecar => write!(f, "sidecar could not be parsed"),
            Self::InputFileRemoved { path } => write!(f, "input file removed: {path}"),
            Self::InputFileUnreadable { path } => {
                write!(f, "input file could not be read: {path}")
            }
            Self::InputFileUntracked { path } => {
                write!(f, "input file has no recorded validation state: {path}")
            }
            Self::InputFileModified {
                path,
                built_at,
                modified_at,
            } => write!(
                f,
                "input file modified: {path} (mtime at build time {built_at}, now {modified_at})"
            ),
            Self::InputFileAdded { path } => write!(f, "input file added: {path}"),
            Self::ArtifactRemoved { path } => {
                write!(f, "cached artifact removed: {}", path.display())
            }
            Self::InputFileUnconfirmed { path } => {
                write!(
                    f,
                    "input file was modified while the entry was being built; rebuilding to confirm it: {path}"
                )
            }
            Self::EntryChanged => {
                write!(f, "entry changed while it was being validated")
            }
        }
    }
}

impl StaleFile {
    /// Maps a snapshot verification failure onto the cache miss reasons.
    fn into_artifact_miss_reason(self) -> CacheMissReason {
        let path = AbsPathBuf::new(self.path).expect("recorded input paths are absolute");
        match self.reason {
            StaleFileReason::Removed => CacheMissReason::InputFileRemoved { path },
            StaleFileReason::Unreadable => CacheMissReason::InputFileUnreadable { path },
            StaleFileReason::Untracked => CacheMissReason::InputFileUntracked { path },
            StaleFileReason::Modified { recorded, observed } => {
                CacheMissReason::InputFileModified {
                    path,
                    built_at: DateTime::<Utc>::from(recorded),
                    modified_at: DateTime::<Utc>::from(observed),
                }
            }
            StaleFileReason::Unconfirmed => CacheMissReason::InputFileUnconfirmed { path },
        }
    }
}

/// Artifact cache rooted at `<cache_root>/artifacts/`.
#[derive(Clone, Debug)]
pub struct ArtifactCache {
    root: PathBuf,
    io_concurrency_semaphore: Option<Arc<Semaphore>>,
}

impl ArtifactCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            io_concurrency_semaphore: None,
        }
    }

    pub fn with_io_concurrency_semaphore(
        mut self,
        io_concurrency_semaphore: Option<Arc<Semaphore>>,
    ) -> Self {
        self.io_concurrency_semaphore = io_concurrency_semaphore;
        self
    }

    /// Directory that holds every entry for `package`.
    pub fn package_dir(&self, package: &PackageName) -> PathBuf {
        self.root.join(package.as_normalized())
    }

    /// Directory that holds the entry for `(package, key)`.
    pub fn entry_dir(&self, package: &PackageName, key: &ArtifactCacheKey) -> PathBuf {
        self.package_dir(package).join(&key.0)
    }

    fn sidecar_path(&self, package: &PackageName, key: &ArtifactCacheKey) -> PathBuf {
        self.entry_dir(package, key).join("sidecar.json")
    }

    fn immutable_alias_path(
        &self,
        package: &PackageName,
        key: &ImmutableArtifactCacheKey,
    ) -> PathBuf {
        self.root
            .join("immutable-v1")
            .join(package.as_normalized())
            .join(format!("{}.json", key.0))
    }

    /// Remove every cached entry for `package`. Silently succeeds if
    /// there's nothing cached yet. Intended for CLI invalidation
    /// commands like `pixi build --clean` or `pixi install
    /// --force-reinstall`.
    pub fn clear_package(&self, package: &PackageName) -> std::io::Result<()> {
        for dir in [
            self.package_dir(package),
            self.root.join("immutable-v1").join(package.as_normalized()),
        ] {
            match fs_err::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Remove every cached entry across every package.
    pub fn clear_all(&self) -> std::io::Result<()> {
        match fs_err::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Path to the entry's cross-process `.lock` file. Callers hold a
    /// read lock on this file while inspecting the sidecar and a write
    /// lock while storing the artifact + sidecar. The file sits inside
    /// the entry directory so it gets removed alongside the entry when
    /// [`clear_package`](Self::clear_package) runs.
    fn lock_path(&self, package: &PackageName, key: &ArtifactCacheKey) -> PathBuf {
        self.entry_dir(package, key).join(".lock")
    }

    /// Open (creating if needed) the entry's lock file.
    async fn open_lock_file(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
    ) -> Result<tokio::fs::File, ArtifactCacheError> {
        let entry_dir = self.entry_dir(package, key);
        tokio::fs::create_dir_all(&entry_dir).await.map_err(|err| {
            ArtifactCacheError::io("creating entry directory", entry_dir.clone(), err)
        })?;
        let lock_path = self.lock_path(package, key);
        tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .await
            .map_err(|err| ArtifactCacheError::io("opening lock file", lock_path, err))
    }

    /// Look up an entry. Returns [`CacheLookup::Miss`] with the reason if the
    /// entry is missing or stale (parse failure, mtime mismatch, missing file,
    /// or a newly-added file matches the recorded globs). For an immutable
    /// source only the first two apply; the file-level checks are skipped
    /// because the cache key already pins the exact source content.
    ///
    /// Holds a shared read lock on the entry's `.lock` file while
    /// reading the sidecar, so a concurrent [`store`](Self::store) (which
    /// takes an exclusive write lock) blocks until we're done. The lock
    /// is dropped before the mtime/glob checks run, since those only touch
    /// files outside the cache entry and don't race with writers.
    pub async fn lookup(
        &self,
        ctx: &mut ComputeCtx,
        package: &PackageName,
        key: &ArtifactCacheKey,
        source_dir: &AbsPath,
        mutability: SourceMutability,
    ) -> Result<CacheLookup, ArtifactCacheError> {
        let sidecar_path = self.sidecar_path(package, key);
        // Fast-path the common "no entry yet" case: skip creating the
        // lock file at all when the sidecar doesn't exist. If the
        // entry directory was never created, neither was the lock.
        if fs_err::metadata(&sidecar_path).is_err() {
            return Ok(CacheLookup::Miss(CacheMissReason::NoEntry));
        }

        let lock_file = self.open_lock_file(package, key).await?;
        let _guard = lock_file.lock_read().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring shared lock",
                self.lock_path(package, key),
                err.error,
            )
        })?;

        let bytes = match tokio::fs::read(&sidecar_path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss(CacheMissReason::NoEntry));
            }
            Err(err) => return Err(ArtifactCacheError::io("reading sidecar", sidecar_path, err)),
        };
        let Ok(mut sidecar) = serde_json::from_slice::<ArtifactSidecar>(&bytes) else {
            // Treat unparsable sidecars as misses; the caller will rebuild
            // and overwrite. This also covers semantically invalid entries:
            // relative path keys fail `AbsPathBuf` deserialization.
            return Ok(CacheLookup::Miss(CacheMissReason::CorruptSidecar));
        };
        drop(_guard);

        // For an immutable source the cache key already pins the exact
        // content, so the per-file checks below would only report noise:
        // a re-cloned git checkout gets fresh mtimes, and a wiped cache
        // dir removes the recorded files entirely. Skipping them also
        // covers sidecars written by older versions, which still carry
        // file lists for immutable sources.
        let mut refreshed = false;
        if mutability == SourceMutability::Mutable {
            // The snapshot check and the glob walk inspect independent
            // aspects of the source tree, so they run concurrently. The walk
            // catches files added since the entry was written.
            let (freshness, current) = {
                let snapshot = sidecar.input_snapshot();
                let files = sidecar.input_files.keys().cloned().collect::<Vec<_>>();
                let verify_future =
                    snapshot.verify(files, None, self.io_concurrency_semaphore.clone());
                let glob_future = crate::input_globs::collect_input_files(
                    ctx,
                    &sidecar.input_glob_sets,
                    source_dir,
                );
                tokio::join!(verify_future, glob_future)
            };

            match freshness {
                SnapshotFreshness::Fresh => {}
                SnapshotFreshness::Refreshed(pairs) => {
                    for (path, fingerprint) in &pairs {
                        sidecar
                            .input_files
                            .insert(path.clone(), DateTime::<Utc>::from(fingerprint.modified));
                    }
                    sidecar.input_file_states.apply_refresh(pairs);
                    refreshed = true;
                }
                SnapshotFreshness::Stale(stale) => {
                    return Ok(CacheLookup::Miss(stale.into_artifact_miss_reason()));
                }
            }

            let current = current.map_err(ArtifactCacheError::Glob)?;
            for matched in current {
                if !sidecar.input_files.contains_key(&matched) {
                    return Ok(CacheLookup::Miss(CacheMissReason::InputFileAdded {
                        path: matched,
                    }));
                }
            }
        }

        let artifact_path = self
            .entry_dir(package, key)
            .join(&sidecar.artifact_filename);
        if fs_err::metadata(&artifact_path).is_err() {
            return Ok(CacheLookup::Miss(CacheMissReason::ArtifactRemoved {
                path: artifact_path,
            }));
        }

        // The lock was dropped during validation, so a concurrent store may
        // have replaced the entry; the recorded sha256 would then describe a
        // different artifact. The refresh write doubles as that check: its
        // byte compare-and-swap only persists over the bytes this probe
        // validated.
        if mutability == SourceMutability::Mutable {
            let unchanged = if refreshed {
                match self
                    .try_refresh_sidecar(package, key, &bytes, &sidecar)
                    .await
                {
                    Ok(SidecarRefresh::Written) => true,
                    Ok(SidecarRefresh::Changed) => false,
                    Err(err) => {
                        // The refresh is best-effort, but its failure leaves
                        // the entry state unknown; re-validate the bytes.
                        tracing::debug!(
                            error = %err,
                            "Failed to persist refreshed artifact input fingerprints; using the verified entry"
                        );
                        self.sidecar_unchanged(package, key, &bytes).await?
                    }
                }
            } else {
                self.sidecar_unchanged(package, key, &bytes).await?
            };
            if !unchanged {
                return Ok(CacheLookup::Miss(CacheMissReason::EntryChanged));
            }
        }

        // Point the record at the artifact being returned. Sidecars written
        // by older versions carry the work-directory output path, which does
        // not survive workspace-cache cleanups.
        let mut record = sidecar.record;
        record.url = Url::from_file_path(&artifact_path).expect("cache entry paths are absolute");

        Ok(CacheLookup::Hit(CachedArtifact {
            artifact: artifact_path,
            sha256: sidecar.artifact_sha256,
            record,
        }))
    }

    /// Open the entry's lock file without creating anything. `None` when the
    /// entry is gone, e.g. after a concurrent `clear_package`.
    async fn open_lock_file_if_exists(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
    ) -> Result<Option<tokio::fs::File>, ArtifactCacheError> {
        let lock_path = self.lock_path(package, key);
        match tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .await
        {
            Ok(file) => Ok(Some(file)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ArtifactCacheError::io("opening lock file", lock_path, err)),
        }
    }

    /// Persist refreshed mtimes and hashes if no other process changed the
    /// sidecar while the files were being inspected.
    async fn try_refresh_sidecar(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
        expected_bytes: &[u8],
        sidecar: &ArtifactSidecar,
    ) -> Result<SidecarRefresh, ArtifactCacheError> {
        let sidecar_path = self.sidecar_path(package, key);
        // Never recreate the entry directory here: a concurrent
        // `clear_package` must not be undone by a refresh.
        let Some(lock_file) = self.open_lock_file_if_exists(package, key).await? else {
            return Ok(SidecarRefresh::Changed);
        };
        let _guard = lock_file.lock_write().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring exclusive lock",
                self.lock_path(package, key),
                err.error,
            )
        })?;

        let current_bytes = match tokio::fs::read(&sidecar_path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SidecarRefresh::Changed);
            }
            Err(err) => {
                return Err(ArtifactCacheError::io("reading sidecar", sidecar_path, err));
            }
        };
        if current_bytes != expected_bytes {
            return Ok(SidecarRefresh::Changed);
        }

        // Re-serializing the parsed sidecar drops fields written by a newer
        // pixi. With the fields-are-only-added policy that costs the newer
        // process a rebuild at worst, never a wrong hit.
        let bytes = serde_json::to_vec(sidecar).expect("sidecar serialization cannot fail");
        tokio::fs::write(&sidecar_path, bytes)
            .await
            .map_err(|err| ArtifactCacheError::io("writing sidecar", sidecar_path, err))?;
        Ok(SidecarRefresh::Written)
    }

    /// Whether the sidecar still holds exactly `expected_bytes`, checked
    /// under a shared lock. `false` when the entry changed or vanished.
    async fn sidecar_unchanged(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
        expected_bytes: &[u8],
    ) -> Result<bool, ArtifactCacheError> {
        let Some(lock_file) = self.open_lock_file_if_exists(package, key).await? else {
            return Ok(false);
        };
        let _guard = lock_file.lock_read().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring shared lock",
                self.lock_path(package, key),
                err.error,
            )
        })?;
        Ok(matches!(
            tokio::fs::read(self.sidecar_path(package, key)).await,
            Ok(current) if current == expected_bytes
        ))
    }

    /// Read the backend description associated with an immutable alias.
    pub(crate) async fn immutable_alias(
        &self,
        package: &PackageName,
        key: &ImmutableArtifactCacheKey,
    ) -> Result<Option<ImmutableArtifactAlias>, ArtifactCacheError> {
        let alias_path = self.immutable_alias_path(package, key);
        let bytes = match tokio::fs::read(&alias_path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(ArtifactCacheError::io(
                    "reading immutable alias",
                    alias_path,
                    err,
                ));
            }
        };
        Ok(serde_json::from_slice(&bytes).ok())
    }

    /// Look up an immutable-source artifact without a source checkout.
    ///
    /// The alias is written only after the normal artifact entry has been
    /// stored. Since a locked Git commit identifies immutable source content,
    /// this intentionally skips the mtime and input-glob validation that
    /// [`Self::lookup`] performs for mutable sources.
    pub async fn lookup_immutable(
        &self,
        package: &PackageName,
        key: &ImmutableArtifactCacheKey,
        backend_identifier: &str,
    ) -> Result<Option<CachedArtifact>, ArtifactCacheError> {
        let Some(alias) = self.immutable_alias(package, key).await? else {
            return Ok(None);
        };
        if alias.backend_identifier != backend_identifier {
            return Ok(None);
        }

        let sidecar_path = self.sidecar_path(package, &alias.artifact_key);
        if fs_err::metadata(&sidecar_path).is_err() {
            return Ok(None);
        }
        let Some(lock_file) = self
            .open_lock_file_if_exists(package, &alias.artifact_key)
            .await?
        else {
            return Ok(None);
        };
        let _guard = lock_file.lock_read().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring shared lock",
                self.lock_path(package, &alias.artifact_key),
                err.error,
            )
        })?;
        let bytes = match tokio::fs::read(&sidecar_path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(ArtifactCacheError::io("reading sidecar", sidecar_path, err)),
        };
        let Ok(sidecar) = serde_json::from_slice::<ArtifactSidecar>(&bytes) else {
            return Ok(None);
        };
        if sidecar.artifact_sha256 != alias.artifact_sha256 {
            return Ok(None);
        }
        let artifact_path = self
            .entry_dir(package, &alias.artifact_key)
            .join(&sidecar.artifact_filename);
        if fs_err::metadata(&artifact_path).is_err() {
            return Ok(None);
        }

        let mut record = sidecar.record;
        record.url = Url::from_file_path(&artifact_path).expect("cache entry paths are absolute");
        Ok(Some(CachedArtifact {
            artifact: artifact_path,
            sha256: sidecar.artifact_sha256,
            record,
        }))
    }

    /// Publish an immutable-source alias for an existing artifact entry.
    ///
    /// The target entry must already have been stored successfully. Readers
    /// treat a malformed or temporarily absent alias as a cache miss.
    pub async fn publish_immutable_alias(
        &self,
        package: &PackageName,
        immutable_key: &ImmutableArtifactCacheKey,
        artifact_key: &ArtifactCacheKey,
        artifact: &CachedArtifact,
        backend_identifier: String,
        backend_spec: JsonRpcBackendSpec,
    ) -> Result<(), ArtifactCacheError> {
        let Some(backend_spec) = ImmutableBackendSpec::from_backend_spec(backend_spec) else {
            return Ok(());
        };
        let alias_path = self.immutable_alias_path(package, immutable_key);
        let parent = alias_path
            .parent()
            .expect("immutable alias path has a parent");
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| ArtifactCacheError::io("creating immutable alias directory", parent.into(), err))?;
        let alias = ImmutableArtifactAlias {
            artifact_key: artifact_key.clone(),
            artifact_sha256: artifact.sha256,
            backend_identifier,
            backend_spec,
        };
        let bytes = serde_json::to_vec(&alias).expect("immutable alias serializes");
        tokio::fs::write(&alias_path, bytes)
            .await
            .map_err(|err| ArtifactCacheError::io("writing immutable alias", alias_path, err))
    }

    /// Place `artifact_source` into the cache and write its sidecar.
    ///
    /// `input_files` are absolute paths; their mtimes are captured at store
    /// time. `record` is the synthesized `RepoDataRecord` for the artifact; it
    /// is persisted in the sidecar so cache hits can skip re-reading
    /// index.json.
    ///
    /// `build_started` is when the backend began reading the sources: a file
    /// modified after it gets an unconfirmed fingerprint (see
    /// `InputSnapshot::capture`).
    ///
    /// Holds an exclusive write lock on the entry's `.lock` file for
    /// the artifact copy + sidecar write, so a concurrent
    /// [`lookup`](Self::lookup) blocks until the new state is fully
    /// committed.
    #[allow(clippy::too_many_arguments)]
    pub async fn store(
        &self,
        package: &PackageName,
        key: &ArtifactCacheKey,
        artifact_source: &Path,
        input_glob_sets: Vec<InputGlobSet>,
        input_files: impl IntoIterator<Item = AbsPathBuf>,
        build_started: SystemTime,
        mut record: RepoDataRecord,
    ) -> Result<CachedArtifact, ArtifactCacheError> {
        let entry_dir = self.entry_dir(package, key);
        let lock_file = self.open_lock_file(package, key).await?;
        let _guard = lock_file.lock_write().await.map_err(|err| {
            ArtifactCacheError::io(
                "acquiring exclusive lock",
                self.lock_path(package, key),
                err.error,
            )
        })?;

        // The entry being replaced supplies the previous observation that
        // can confirm an unconfirmed fingerprint.
        let previous_snapshot = tokio::fs::read(self.sidecar_path(package, key))
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ArtifactSidecar>(&bytes).ok())
            .map(|sidecar| sidecar.input_file_states);

        let filename = artifact_source
            .file_name()
            .ok_or_else(|| ArtifactCacheError::ArtifactFilename(artifact_source.to_path_buf()))?
            .to_string_lossy()
            .into_owned();
        let dest = entry_dir.join(&filename);

        // Copy rather than move: the source may live outside our cache root
        // (e.g. a backend-managed work directory), and a copy keeps failure
        // semantics simple even across filesystems.
        tokio::fs::copy(artifact_source, &dest)
            .await
            .map_err(|err| {
                ArtifactCacheError::io("copying artifact into cache", dest.clone(), err)
            })?;

        // The record was synthesized from the freshly-built artifact and
        // points at its work-directory location. Persist and return the
        // cached copy instead; the work directory is mutable and may be
        // cleaned before the record is consumed again.
        record.url = Url::from_file_path(&dest).expect("cache entry paths are absolute");

        let digest_path = dest.clone();
        let artifact_digest =
            spawn_blocking_with_io_permit(self.io_concurrency_semaphore.clone(), move || {
                rattler_digest::compute_file_digest::<rattler_digest::Sha256>(&digest_path)
            });

        // A file that cannot be hashed keeps mtime-only validation, and a
        // file that vanished is dropped; the glob check treats it as new if
        // it reappears. Neither fails the store.
        let snapshot_future = InputSnapshot::capture(
            input_files,
            Some(build_started),
            previous_snapshot.as_ref(),
            self.io_concurrency_semaphore.clone(),
        );
        let (sha256, snapshot) = tokio::join!(artifact_digest, snapshot_future);
        let sha256 = sha256
            .expect("sha256 task panicked")
            .map_err(|err| ArtifactCacheError::io("hashing artifact", dest.clone(), err))?;

        let sidecar =
            ArtifactSidecar::new(input_glob_sets, snapshot, sha256, filename, record.clone());
        let sidecar_path = self.sidecar_path(package, key);
        let bytes = serde_json::to_vec(&sidecar).expect("sidecar serialization cannot fail");
        tokio::fs::write(&sidecar_path, &bytes)
            .await
            .map_err(|err| ArtifactCacheError::io("writing sidecar", sidecar_path.clone(), err))?;

        Ok(CachedArtifact {
            artifact: dest,
            sha256,
            record,
        })
    }
}

#[derive(Debug, Error, Clone)]
pub enum ArtifactCacheError {
    #[error("{operation} at {}", path.display())]
    Io {
        operation: String,
        path: PathBuf,
        #[source]
        source: Arc<std::io::Error>,
    },

    #[error(transparent)]
    Glob(Arc<pixi_glob::GlobSetError>),

    #[error("artifact path has no filename: {}", .0.display())]
    ArtifactFilename(PathBuf),
}

impl ArtifactCacheError {
    fn io(operation: impl Into<String>, path: PathBuf, err: std::io::Error) -> Self {
        Self::Io {
            operation: operation.into(),
            path,
            source: Arc::new(err),
        }
    }
}

impl From<pixi_glob::GlobSetError> for ArtifactCacheError {
    fn from(err: pixi_glob::GlobSetError) -> Self {
        Self::Glob(Arc::new(err))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, str::FromStr, time::SystemTime};

    use pixi_compute_engine::ComputeEngine;
    use rattler_conda_types::{PackageName, PackageRecord};
    use tempfile::TempDir;

    use super::*;

    fn key(s: &str) -> ArtifactCacheKey {
        ArtifactCacheKey(s.to_string())
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::from_str(name).unwrap()
    }

    /// A single glob group with default (markers-free, hidden-excluding) config.
    fn glob_group(patterns: &[&str]) -> InputGlobSet {
        InputGlobSet {
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
            markers: Vec::new(),
            exclude_hidden: true,
            root: None,
        }
    }

    fn abs(path: impl Into<PathBuf>) -> AbsPathBuf {
        AbsPathBuf::new(path).unwrap()
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn mtime(path: &Path) -> SystemTime {
        fs_err::metadata(path).unwrap().modified().unwrap()
    }

    /// Moves the mtime forward by `secs` and returns the new mtime.
    fn touch(path: &Path, secs: u64) -> SystemTime {
        let modified = mtime(path) + std::time::Duration::from_secs(secs);
        set_modified(path, modified);
        modified
    }

    /// Drive [`ArtifactCache::lookup`] (which needs a `ComputeCtx`) through the
    /// provided engine. The test source dirs are absolute tempdirs.
    async fn lookup(
        engine: &ComputeEngine,
        cache: &ArtifactCache,
        package: &PackageName,
        key: &ArtifactCacheKey,
        source: &Path,
        mutability: SourceMutability,
    ) -> Result<CacheLookup, ArtifactCacheError> {
        let source = AbsPath::new(source).unwrap();
        engine
            .with_ctx(async |ctx| cache.lookup(ctx, package, key, source, mutability).await)
            .await
            .expect("compute engine cycle")
    }

    /// Unwrap a lookup that is expected to miss, yielding its reason.
    fn expect_miss(lookup: CacheLookup) -> CacheMissReason {
        match lookup {
            CacheLookup::Miss(reason) => reason,
            CacheLookup::Hit(hit) => {
                panic!(
                    "expected a cache miss, got a hit on {}",
                    hit.artifact.display()
                )
            }
        }
    }

    /// The hit, if the lookup was one.
    fn as_hit(lookup: CacheLookup) -> Option<CachedArtifact> {
        match lookup {
            CacheLookup::Hit(hit) => Some(hit),
            CacheLookup::Miss(_) => None,
        }
    }

    fn dummy_record(name: &str) -> RepoDataRecord {
        let mut pr = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        RepoDataRecord {
            package_record: pr,
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                &format!("{name}-1.0.0-h0.conda"),
            )
            .unwrap(),
            url: url::Url::parse(&format!("file:///{name}-1.0.0-h0.conda")).unwrap(),
            channel: None,
        }
    }

    #[tokio::test]
    async fn lookup_missing_entry_is_a_miss() {
        let f = Fixture::new();
        assert!(matches!(f.lookup_miss().await, CacheMissReason::NoEntry));
    }

    #[tokio::test]
    async fn round_trip_store_then_lookup() {
        let f = Fixture::new();
        let input = f.write("main.py", b"print(1)");
        let stored = f
            .store(vec![glob_group(&["**/*.py"])], vec![abs(input)])
            .await;

        let hit = f.lookup().await.expect("cache hit after store");
        assert_eq!(hit.sha256, stored.sha256);
        assert_eq!(hit.artifact, stored.artifact);
        assert_eq!(
            hit.record.package_record.name,
            stored.record.package_record.name
        );
    }

    #[tokio::test]
    async fn lookup_hashes_source_file_when_mtime_changes() {
        let (f, input) = Fixture::with_input("main.py", b"old").await;

        // Touching the input without changing its contents keeps the cache
        // valid and refreshes the stored mtime.
        let touched_mtime = touch(&input, 1);
        assert!(
            f.lookup().await.is_some(),
            "an mtime-only change should remain cached"
        );
        assert_eq!(
            f.sidecar()
                .input_file_states
                .get(&input)
                .unwrap()
                .modified(),
            touched_mtime,
            "the refreshed mtime should be persisted"
        );

        // A same-size content change still invalidates the entry.
        fs_err::write(&input, b"new").unwrap();
        touch(&input, 1);
        assert!(
            matches!(
                f.lookup_miss().await,
                CacheMissReason::InputFileModified { ref path, .. } if path.as_std_path() == input,
            ),
            "mtime change should invalidate the entry and name the file",
        );
    }

    /// An immutable source (git commit, url archive) is fully identified by
    /// the cache key; the recorded input files may be long gone (e.g. the
    /// global cache dir holding the git checkout was wiped) without making
    /// the entry stale. This also covers sidecars written by older versions,
    /// which still carry file lists for immutable sources.
    #[tokio::test]
    async fn lookup_immutable_hits_despite_deleted_input_files() {
        let (f, input) = Fixture::with_input("main.py", b"body").await;

        // Simulate a wiped cache dir: every recorded input file is gone.
        fs_err::remove_file(&input).unwrap();

        assert!(
            f.lookup_with(SourceMutability::Immutable).await.is_some(),
            "an immutable source must hit even when the recorded input files are gone",
        );
        assert!(
            matches!(
                f.lookup_miss().await,
                CacheMissReason::InputFileRemoved { .. }
            ),
            "a mutable source with deleted input files must stay a miss",
        );
    }

    #[tokio::test]
    async fn immutable_alias_reuses_artifact_without_source_tree() {
        let tmp = TempDir::new().unwrap();
        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let source = tmp.path().join("src");
        fs_err::create_dir_all(&source).unwrap();
        let input = source.join("main.py");
        fs_err::write(&input, b"print(1)").unwrap();

        let artifact_source = tmp.path().join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact_source, b"pretend this is a conda").unwrap();
        let artifact_key = key("linux-64-abc");
        let stored = cache
            .store(
                &pkg("foo"),
                &artifact_key,
                &artifact_source,
                vec![glob_group(&["**/*.py"])],
                vec![abs(&input)],
                SystemTime::now(),
                dummy_record("foo"),
            )
            .await
            .unwrap();
        let immutable_key = ImmutableArtifactCacheKey("immutable-key".to_string());
        cache
            .publish_immutable_alias(
                &pkg("foo"),
                &immutable_key,
                &artifact_key,
                &stored,
                "test-backend".to_string(),
                JsonRpcBackendSpec::default_rattler_build(Vec::new()),
            )
            .await
            .unwrap();

        assert!(cache
            .lookup_immutable(&pkg("foo"), &immutable_key, "other-backend")
            .await
            .unwrap()
            .is_none());
        fs_err::remove_dir_all(&source).unwrap();

        let hit = cache
            .lookup_immutable(&pkg("foo"), &immutable_key, "test-backend")
            .await
            .unwrap()
            .expect("immutable alias hit");
        assert_eq!(hit.sha256, stored.sha256);
        assert_eq!(hit.artifact, stored.artifact);

        cache.clear_package(&pkg("foo")).unwrap();
        assert!(cache
            .lookup_immutable(&pkg("foo"), &immutable_key, "test-backend")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn lookup_stale_when_new_file_matches_glob() {
        let (f, _input) = Fixture::with_input("main.py", b"body").await;

        // Introduce a new file that matches the stored glob.
        f.write("extra.py", b"new module");
        assert!(
            matches!(
                f.lookup_miss().await,
                CacheMissReason::InputFileAdded { ref path } if path.as_std_path().ends_with("extra.py"),
            ),
            "a newly-added matching file should invalidate the entry and name the file"
        );
    }

    #[tokio::test]
    async fn sidecar_preserves_record_fields_across_lookup() {
        // Verify that every RepoDataRecord field we care about
        // (identifier, package_record.version, .build, .subdir, .depends)
        // round-trips through the sidecar, and that the url is rewritten to
        // the cached artifact. A future refactor that accidentally drops a
        // field will fail this test.
        let f = Fixture::new();
        let mut record = dummy_record("foo");
        record.package_record.depends = vec!["python >=3.8".into(), "numpy".into()];
        record.package_record.build_number = 42;
        record.url = url::Url::parse("https://example.test/foo-1.0.0-h0.conda").unwrap();

        f.cache
            .store(
                &pkg("foo"),
                &key("linux-64-abc"),
                &f.artifact,
                Vec::new(),
                Vec::<AbsPathBuf>::new(),
                SystemTime::now(),
                record.clone(),
            )
            .await
            .unwrap();

        let hit = f.lookup().await.expect("cache should hit");

        // Package record identity.
        assert_eq!(hit.record.package_record.name, record.package_record.name);
        assert_eq!(
            hit.record.package_record.version,
            record.package_record.version
        );
        assert_eq!(hit.record.package_record.build, record.package_record.build);
        assert_eq!(
            hit.record.package_record.subdir,
            record.package_record.subdir
        );
        assert_eq!(hit.record.package_record.build_number, 42);
        assert_eq!(
            hit.record.package_record.depends,
            record.package_record.depends
        );

        // RepoDataRecord top-level fields. The url always points at the
        // cached artifact, replacing whatever location the record carried
        // when it was stored.
        assert_eq!(
            hit.record.url,
            url::Url::from_file_path(&hit.artifact).unwrap()
        );
        assert_eq!(hit.record.identifier, record.identifier);
    }

    /// A second `store` concurrent with a `lookup` must see a consistent
    /// view: either the pre-existing entry or the freshly-written one,
    /// never a mix of the two. We can't easily assert interleaving here
    /// in a deterministic way, so we smoke-test the lock path by
    /// spawning a lot of concurrent lookups + stores and asserting no
    /// IO errors fall out.
    #[tokio::test]
    async fn concurrent_store_and_lookup_do_not_error() {
        let f = Fixture::new();
        let input = f.write("main.py", b"body");
        let engine = ComputeEngine::new();

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache = f.cache.clone();
            let input = input.clone();
            let artifact = f.artifact.clone();
            handles.push(tokio::spawn(async move {
                cache
                    .store(
                        &pkg("foo"),
                        &key("linux-64-abc"),
                        &artifact,
                        Vec::new(),
                        vec![abs(input)],
                        SystemTime::now(),
                        dummy_record("foo"),
                    )
                    .await
                    .unwrap();
            }));
        }
        for _ in 0..8 {
            let cache = f.cache.clone();
            let engine = engine.clone();
            let source = f.source.clone();
            handles.push(tokio::spawn(async move {
                // Ignore whether it's a hit or miss; racing with stores
                // the lookup may see either. The contract under test is
                // that it never errors out.
                let _ = lookup(
                    &engine,
                    &cache,
                    &pkg("foo"),
                    &key("linux-64-abc"),
                    &source,
                    SourceMutability::Mutable,
                )
                .await
                .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn corrupt_sidecar_is_a_miss() {
        let f = Fixture::new();
        let entry = f.cache.entry_dir(&pkg("foo"), &key("linux-64-abc"));
        fs_err::create_dir_all(&entry).unwrap();
        fs_err::write(entry.join("sidecar.json"), b"{ this is not valid").unwrap();

        assert!(matches!(
            f.lookup_miss().await,
            CacheMissReason::CorruptSidecar
        ));
    }

    /// A relative key under `input_file_states` fails `AbsPathBuf`
    /// deserialization, so the whole sidecar counts as corrupt instead of
    /// reaching any code that assumes absolute paths.
    #[tokio::test]
    async fn a_sidecar_with_a_relative_state_key_is_a_corrupt_miss() {
        let (f, input) = Fixture::with_input("main.py", b"body").await;

        let mut json = f.sidecar_json();
        let states = json
            .pointer_mut("/input_file_states/files")
            .unwrap()
            .as_object_mut()
            .unwrap();
        let state = states.values().next().unwrap().clone();
        states.insert("relative.py".into(), state);
        f.write_sidecar_json(&json);

        // A touch forces the refresh rewrite that would hit the bad key.
        touch(&input, 1);
        assert!(matches!(
            f.lookup_miss().await,
            CacheMissReason::CorruptSidecar
        ));
    }

    /// Regression for prefix-dev/pixi#6232: a thin package whose manifest
    /// points at `../recipe.yaml` makes the build report a `../**` input
    /// glob. The walker rebases that onto the parent directory, so it matches
    /// files outside the package's own source dir (the recipe, the build
    /// script, sibling variant dirs). Those must round-trip through store +
    /// lookup so the second run is a cache hit instead of rebuilding from
    /// scratch.
    #[tokio::test]
    async fn issue_6232_parent_recipe_glob_does_not_force_rebuild() {
        let tmp = TempDir::new().unwrap();
        // Layout: <root>/llama-cpp/{recipe.yaml, build.sh, vulkan/{pixi.toml, variants.yaml}}
        let parent = tmp.path().join("llama-cpp");
        let source = parent.join("vulkan");
        fs_err::create_dir_all(&source).unwrap();
        fs_err::write(parent.join("recipe.yaml"), b"recipe").unwrap();
        fs_err::write(parent.join("build.sh"), b"script").unwrap();
        fs_err::write(source.join("pixi.toml"), b"manifest").unwrap();
        fs_err::write(source.join("variants.yaml"), b"backend:\n  - vulkan\n").unwrap();
        let source_abs = AbsPath::new(&source).unwrap();

        let cache = ArtifactCache::new(tmp.path().join("artifacts"));
        let engine = ComputeEngine::new();
        let scratch = tmp.path().join("scratch");
        fs_err::create_dir_all(&scratch).unwrap();
        let artifact = scratch.join("foo-1.0.0-h0.conda");
        fs_err::write(&artifact, b"artifact").unwrap();

        // The build reports `../**` because the recipe lives in the parent dir.
        let groups = vec![glob_group(&["../**"])];

        // Resolve the matched files exactly as the source-build pipeline does.
        let input_files = engine
            .with_ctx(async |ctx| {
                crate::input_globs::collect_input_files(ctx, &groups, source_abs).await
            })
            .await
            .unwrap()
            .unwrap();
        // Sanity: the `../**` glob really did reach the parent recipe.
        assert!(
            input_files
                .iter()
                .any(|p| p.as_std_path().ends_with("llama-cpp/recipe.yaml")),
            "expected `../**` to match the parent recipe, got {input_files:?}",
        );

        let key = key("linux-64-abc");
        cache
            .store(
                &pkg("foo"),
                &key,
                &artifact,
                groups,
                input_files,
                SystemTime::now(),
                dummy_record("foo"),
            )
            .await
            .unwrap();

        // Second run with nothing changed: must hit, not rebuild.
        let hit = lookup(
            &engine,
            &cache,
            &pkg("foo"),
            &key,
            &source,
            SourceMutability::Mutable,
        )
        .await
        .unwrap();
        assert!(
            matches!(hit, CacheLookup::Hit(_)),
            "issue #6232: a `../`-recipe glob must not force a rebuild on the next run",
        );
    }

    /// Everything a store + lookup round trip needs: a source dir, a fake
    /// `.conda` outside the cache, and the cache itself.
    struct Fixture {
        _tmp: TempDir,
        cache: ArtifactCache,
        source: PathBuf,
        artifact: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let cache = ArtifactCache::new(tmp.path().join("artifacts"));
            let source = tmp.path().join("src");
            fs_err::create_dir_all(&source).unwrap();
            let scratch = tmp.path().join("scratch");
            fs_err::create_dir_all(&scratch).unwrap();
            let artifact = scratch.join("foo-1.0.0-h0.conda");
            fs_err::write(&artifact, b"pretend this is a conda").unwrap();
            Self {
                _tmp: tmp,
                cache,
                source,
                artifact,
            }
        }

        /// Fixture with a single stored input file matched by `**/*.py`.
        async fn with_input(name: &str, contents: &[u8]) -> (Self, PathBuf) {
            let fixture = Self::new();
            let input = fixture.write(name, contents);
            fixture
                .store(vec![glob_group(&["**/*.py"])], vec![abs(input.clone())])
                .await;
            (fixture, input)
        }

        /// Writes `contents` to `name` inside the source dir.
        fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.source.join(name);
            fs_err::write(&path, contents).unwrap();
            path
        }

        async fn store(&self, globs: Vec<InputGlobSet>, inputs: Vec<AbsPathBuf>) -> CachedArtifact {
            self.store_with_build_started(globs, inputs, SystemTime::now())
                .await
        }

        async fn store_with_build_started(
            &self,
            globs: Vec<InputGlobSet>,
            inputs: Vec<AbsPathBuf>,
            build_started: SystemTime,
        ) -> CachedArtifact {
            self.cache
                .store(
                    &pkg("foo"),
                    &key("linux-64-abc"),
                    &self.artifact,
                    globs,
                    inputs,
                    build_started,
                    dummy_record("foo"),
                )
                .await
                .unwrap()
        }

        /// Each call gets a fresh engine, modelling a separate pixi run.
        /// Reusing one engine would reuse its memoized glob walk (see
        /// [`crate::input_globs`]) and hide newly-added files.
        async fn lookup(&self) -> Option<CachedArtifact> {
            self.lookup_with(SourceMutability::Mutable).await
        }

        async fn lookup_with(&self, mutability: SourceMutability) -> Option<CachedArtifact> {
            as_hit(self.lookup_raw(mutability).await)
        }

        /// The miss reason, panicking on a hit.
        async fn lookup_miss(&self) -> CacheMissReason {
            expect_miss(self.lookup_raw(SourceMutability::Mutable).await)
        }

        async fn lookup_raw(&self, mutability: SourceMutability) -> CacheLookup {
            lookup(
                &ComputeEngine::new(),
                &self.cache,
                &pkg("foo"),
                &key("linux-64-abc"),
                &self.source,
                mutability,
            )
            .await
            .unwrap()
        }

        fn sidecar_path(&self) -> PathBuf {
            self.cache.sidecar_path(&pkg("foo"), &key("linux-64-abc"))
        }

        fn sidecar(&self) -> ArtifactSidecar {
            serde_json::from_slice(&fs_err::read(self.sidecar_path()).unwrap()).unwrap()
        }

        fn sidecar_json(&self) -> serde_json::Value {
            serde_json::from_slice(&fs_err::read(self.sidecar_path()).unwrap()).unwrap()
        }

        fn write_sidecar_json(&self, value: &serde_json::Value) {
            fs_err::write(self.sidecar_path(), serde_json::to_vec(value).unwrap()).unwrap();
        }
    }

    /// The cache treats an unchanged (size, mtime) pair as proof that the
    /// contents are unchanged, so a same-size edit that also restores the
    /// original mtime is invisible. This is the documented fast path, not a
    /// regression: without it every probe would have to hash every input.
    /// Recorded here so a future change to `compare_metadata` is a conscious
    /// one.
    #[tokio::test]
    async fn same_size_edit_with_restored_mtime_is_not_detected() {
        let (f, input) = Fixture::with_input("main.py", b"old").await;

        let original_mtime = mtime(&input);
        fs_err::write(&input, b"new").unwrap();
        set_modified(&input, original_mtime);

        assert!(
            f.lookup().await.is_some(),
            "known limitation: an edit that preserves both size and mtime is not detected",
        );
    }

    /// A sidecar written before fingerprints existed carries only mtimes.
    /// Its files keep mtime-only validation: an untouched file hits, and no
    /// hash is ever recorded for it. Hashing the current contents would let
    /// a same-size edit with a preserved mtime vouch for outputs built from
    /// different contents, permanently.
    #[tokio::test]
    async fn legacy_sidecar_without_fingerprints_is_never_upgraded() {
        let (f, input) = Fixture::with_input("main.py", b"body").await;

        // Strip the states to emulate an entry written by an older pixi.
        let mut json = f.sidecar_json();
        json.as_object_mut().unwrap().remove("input_file_states");
        f.write_sidecar_json(&json);
        assert!(f.sidecar().input_file_states.is_empty());

        assert!(
            f.lookup().await.is_some(),
            "a legacy sidecar with untouched files must still hit",
        );
        assert!(
            f.sidecar().input_file_states.is_empty(),
            "a hit must not record a hash it cannot vouch for",
        );

        // A touch costs one rebuild; only a fresh store records fingerprints.
        touch(&input, 5);
        assert!(
            f.lookup().await.is_none(),
            "a legacy entry cannot prove a touched file is unchanged",
        );
        assert!(
            f.sidecar().input_file_states.is_empty(),
            "a miss must not poison the sidecar with a hash of the new contents",
        );
    }

    /// The store-time glob walk and the fingerprint pass are separate, so a
    /// file can vanish in between. The store must succeed, and the resulting
    /// entry must not claim a file it never fingerprinted.
    #[tokio::test]
    async fn store_tolerates_an_input_file_that_vanished_before_fingerprinting() {
        let f = Fixture::new();
        let present = f.write("main.py", b"body");
        let vanished = f.source.join("gone.py");

        f.store(
            vec![glob_group(&["**/*.py"])],
            vec![abs(present.clone()), abs(vanished.clone())],
        )
        .await;

        let sidecar = f.sidecar();
        assert!(
            !sidecar.input_files.contains_key(&abs(vanished.clone())),
            "a file that could not be stat'ed must be dropped from the record",
        );
        assert!(
            f.lookup().await.is_some(),
            "a dropped file must not turn every later lookup into a miss",
        );

        // If it comes back, the glob walk sees a file the entry does not know.
        f.write("gone.py", b"back");
        assert!(
            f.lookup().await.is_none(),
            "a reappearing input file must invalidate the entry",
        );
    }

    /// The new-file check depends on the glob walk, which the compute engine
    /// memoizes for its lifetime. Two lookups sharing an engine therefore
    /// share one walk, and a file added in between is invisible to the
    /// second. This predates fingerprinting and only bounds detection to a
    /// single pixi run; recorded so it is not mistaken for a fingerprint bug.
    #[tokio::test]
    async fn glob_walk_is_memoized_per_engine_so_one_run_sees_one_file_set() {
        let (f, _input) = Fixture::with_input("main.py", b"body").await;

        let engine = ComputeEngine::new();
        let probe = async |source: &Path| {
            as_hit(
                lookup(
                    &engine,
                    &f.cache,
                    &pkg("foo"),
                    &key("linux-64-abc"),
                    source,
                    SourceMutability::Mutable,
                )
                .await
                .unwrap(),
            )
        };

        assert!(probe(&f.source).await.is_some());
        f.write("extra.py", b"new module");
        assert!(
            probe(&f.source).await.is_some(),
            "within one engine the memoized walk still reports the old file set",
        );
        assert!(
            f.lookup().await.is_none(),
            "the next run walks again and must see the new file",
        );
    }

    /// Input globs can match a directory. Hashing one is impossible, so it
    /// must fall back to mtime-only validation instead of failing the store
    /// or hanging.
    #[tokio::test]
    async fn directory_input_falls_back_to_mtime_only_validation() {
        let f = Fixture::new();
        let dir = f.source.join("assets");
        fs_err::create_dir_all(&dir).unwrap();
        let input = f.write("main.py", b"body");

        f.store(Vec::new(), vec![abs(input), abs(dir.clone())])
            .await;

        let sidecar = f.sidecar();
        assert!(
            sidecar.input_files.contains_key(&abs(dir.clone())),
            "a directory should keep timestamp-based validation",
        );
        assert!(
            matches!(
                sidecar.input_file_states.get(&dir),
                Some(InputFileState::MtimeOnly(_))
            ),
            "a directory must never be hashed",
        );
        assert!(f.lookup().await.is_some(), "a directory input must hit");
    }

    /// Sidecars gain fields over time. An entry written by a newer pixi must
    /// still be usable, and the byte-CAS refresh must not corrupt it. The
    /// refresh write does drop the unknown fields; that costs the newer
    /// process a rebuild at worst, never a wrong hit.
    #[tokio::test]
    async fn sidecar_with_unknown_fields_hits_and_only_loses_them_on_refresh() {
        let (f, input) = Fixture::with_input("main.py", b"body").await;

        let mut json = f.sidecar_json();
        json.as_object_mut()
            .unwrap()
            .insert("future_field".into(), serde_json::json!({"a": 1}));
        f.write_sidecar_json(&json);

        // Nothing changed, so no refresh happens and the field survives.
        assert!(
            f.lookup().await.is_some(),
            "unknown fields must not break parsing"
        );
        assert!(
            f.sidecar_json().get("future_field").is_some(),
            "a lookup that refreshes nothing must leave the sidecar bytes alone",
        );

        // A touch forces a refresh, which rewrites the sidecar from the
        // fields this version knows about.
        touch(&input, 3);
        assert!(f.lookup().await.is_some(), "a touch must still hit");
        assert!(
            f.sidecar_json().get("future_field").is_none(),
            "the refresh rewrite drops fields this version does not know",
        );
    }

    /// A build that rewrites a source-tree file on every run makes its mtime
    /// change on every probe. Each probe must re-hash, keep the hit, and
    /// leave the sidecar converged on the mtime it just observed.
    #[tokio::test]
    async fn a_file_touched_before_every_probe_keeps_hitting_and_converges() {
        let (f, input) = Fixture::with_input("generated.py", b"body").await;

        for round in 1..=3u64 {
            let touched = touch(&input, 1);
            assert!(
                f.lookup().await.is_some(),
                "round {round}: a touched-but-unchanged file must stay cached",
            );
            assert_eq!(
                f.sidecar()
                    .input_file_states
                    .get(&input)
                    .unwrap()
                    .modified(),
                touched,
                "round {round}: the refreshed mtime should be persisted",
            );
        }
    }

    /// Timestamps before the unix epoch are representable on both NTFS and
    /// most unix filesystems and must survive the sidecar round trip.
    #[tokio::test]
    async fn pre_epoch_input_mtime_round_trips_through_store_and_lookup() {
        let f = Fixture::new();
        let input = f.write("main.py", b"body");

        let pre_epoch = SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(86_400);
        let settable = OpenOptions::new()
            .write(true)
            .open(&input)
            .unwrap()
            .set_modified(pre_epoch)
            .is_ok();
        if !settable {
            // Some filesystems clamp timestamps to the epoch; nothing to test.
            return;
        }

        f.store(vec![glob_group(&["**/*.py"])], vec![abs(input.clone())])
            .await;
        assert_eq!(
            f.sidecar()
                .input_file_states
                .get(&input)
                .unwrap()
                .modified(),
            pre_epoch,
        );
        let persisted = f.sidecar_json()["input_files"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            persisted.starts_with("1969-12-31T"),
            "timestamps persist as RFC3339, which represents pre-epoch instants; got {persisted}",
        );
        assert!(
            f.lookup().await.is_some(),
            "a pre-epoch input mtime must not invalidate the entry",
        );
    }

    /// A zero-byte input is a degenerate hash case: every empty file has the
    /// same digest. Growing it must invalidate, and emptying it again must
    /// bring the entry back rather than wedging it.
    #[tokio::test]
    async fn zero_byte_input_file_round_trips() {
        let (f, input) = Fixture::with_input("__init__.py", b"").await;
        assert!(f.lookup().await.is_some(), "an empty input file must hit");

        let base = mtime(&input);
        fs_err::write(&input, b"x").unwrap();
        set_modified(&input, base + std::time::Duration::from_secs(1));
        assert!(
            f.lookup().await.is_none(),
            "growing the file must invalidate the entry",
        );

        fs_err::write(&input, b"").unwrap();
        set_modified(&input, base + std::time::Duration::from_secs(2));
        assert!(
            f.lookup().await.is_some(),
            "restoring the empty contents must restore the hit",
        );
    }

    /// Restoring a file from a backup or checking it out again can move its
    /// mtime backwards. The hash, not the direction of the mtime change,
    /// decides: identical contents keep the hit, different contents do not.
    #[tokio::test]
    async fn an_mtime_moving_backwards_is_decided_by_the_hash() {
        let (f, input) = Fixture::with_input("main.py", b"old").await;

        let older = mtime(&input) - std::time::Duration::from_secs(3600);
        set_modified(&input, older);
        assert!(
            f.lookup().await.is_some(),
            "an older mtime with identical contents must stay cached",
        );

        fs_err::write(&input, b"new").unwrap();
        set_modified(&input, older - std::time::Duration::from_secs(3600));
        assert!(
            f.lookup().await.is_none(),
            "an older mtime with changed contents must invalidate",
        );
    }

    /// Grow a file and truncate it back to its original length with different
    /// contents. Only the hash can tell these apart.
    #[tokio::test]
    async fn grow_then_truncate_to_the_original_size_invalidates() {
        let (f, input) = Fixture::with_input("main.py", b"aaa").await;

        fs_err::write(&input, b"bbbbbbbbb").unwrap();
        fs_err::write(&input, b"ccc").unwrap();
        touch(&input, 1);

        assert!(
            f.lookup().await.is_none(),
            "same size, different contents must invalidate",
        );
    }

    /// A file modified after the build started (future-dated mtime, or the
    /// build rewriting it) gets an unconfirmed fingerprint: one more rebuild,
    /// then the stable content is promoted and the entry hits again.
    #[tokio::test]
    async fn unconfirmed_input_converges_after_one_rebuild() {
        let f = Fixture::new();
        let input = f.write("main.py", b"body");
        set_modified(
            &input,
            SystemTime::now() + std::time::Duration::from_secs(3600),
        );

        f.store(vec![glob_group(&["**/*.py"])], vec![abs(input.clone())])
            .await;
        assert!(
            f.lookup().await.is_none(),
            "an unconfirmed fingerprint cannot vouch for the entry",
        );

        // The rebuild observes the same content and promotes it.
        f.store(vec![glob_group(&["**/*.py"])], vec![abs(input.clone())])
            .await;
        assert!(
            f.lookup().await.is_some(),
            "content stable across two builds must be trusted",
        );
    }

    /// A refresh racing a `clear_package` must not write the sidecar back;
    /// explicit invalidation wins.
    #[tokio::test]
    async fn refresh_does_not_resurrect_a_cleared_entry() {
        let (f, _input) = Fixture::with_input("main.py", b"body").await;

        let key = key("linux-64-abc");
        let sidecar_path = f.cache.sidecar_path(&pkg("foo"), &key);
        let bytes = fs_err::read(&sidecar_path).unwrap();
        let sidecar: ArtifactSidecar = serde_json::from_slice(&bytes).unwrap();

        f.cache.clear_package(&pkg("foo")).unwrap();
        f.cache
            .try_refresh_sidecar(&pkg("foo"), &key, &bytes, &sidecar)
            .await
            .unwrap();

        assert!(
            fs_err::metadata(f.cache.entry_dir(&pkg("foo"), &key)).is_err(),
            "a refresh must not recreate a cleared entry directory",
        );
    }

    /// The promotion that trusts an unconfirmed fingerprint lives entirely in
    /// the entry being replaced. Clearing the package throws that observation
    /// away, so the next build is a cold one again: unconfirmed, one rebuild,
    /// then trusted.
    #[tokio::test]
    async fn clearing_the_package_resets_the_unconfirmed_promotion() {
        let f = Fixture::new();
        let input = f.write("main.py", b"body");
        set_modified(
            &input,
            SystemTime::now() + std::time::Duration::from_secs(3600),
        );
        let store = async || {
            f.store(vec![glob_group(&["**/*.py"])], vec![abs(input.clone())])
                .await;
        };

        store().await;
        assert!(f.lookup().await.is_none());
        store().await;
        assert!(
            f.lookup().await.is_some(),
            "the second build promotes the stable content",
        );

        f.cache.clear_package(&pkg("foo")).unwrap();
        store().await;
        assert!(
            f.lookup().await.is_none(),
            "with the previous observation cleared the entry is cold again",
        );
        store().await;
        assert!(
            f.lookup().await.is_some(),
            "and it converges again after one rebuild",
        );
    }

    /// An unconfirmed fingerprint misses with its own reason that names the
    /// file, instead of an `InputFileModified` claiming a change from an
    /// instant to that same instant.
    #[tokio::test]
    async fn an_unconfirmed_input_misses_with_a_dedicated_reason() {
        let f = Fixture::new();
        let input = f.write("main.py", b"body");
        let observed = mtime(&input);

        f.store_with_build_started(
            vec![glob_group(&["**/*.py"])],
            vec![abs(input.clone())],
            observed - std::time::Duration::from_secs(10),
        )
        .await;

        match f.lookup_miss().await {
            CacheMissReason::InputFileUnconfirmed { path } => {
                assert_eq!(path.as_std_path(), input, "the miss must name the file");
            }
            other => panic!("expected an input-file-unconfirmed miss, got {other:?}"),
        }
    }

    /// The refresh write is a compare-and-swap on the sidecar bytes: another
    /// process that rewrote the entry in the meantime must win.
    #[tokio::test]
    async fn refresh_does_not_overwrite_a_sidecar_that_moved() {
        let (f, _input) = Fixture::with_input("main.py", b"body").await;

        let key = key("linux-64-abc");
        let stale_bytes = fs_err::read(f.sidecar_path()).unwrap();
        let mut sidecar: ArtifactSidecar = serde_json::from_slice(&stale_bytes).unwrap();
        sidecar.artifact_filename = "refreshed.conda".into();

        // Emulate a concurrent store landing between the read and the refresh.
        let mut json = f.sidecar_json();
        json.as_object_mut()
            .unwrap()
            .insert("artifact_filename".into(), "concurrent.conda".into());
        f.write_sidecar_json(&json);

        f.cache
            .try_refresh_sidecar(&pkg("foo"), &key, &stale_bytes, &sidecar)
            .await
            .unwrap();

        assert_eq!(
            f.sidecar().artifact_filename,
            "concurrent.conda",
            "the refresh must not clobber the entry another writer committed",
        );
    }

    /// An entry whose lock file is gone can no longer be re-validated, which
    /// is the condition `lookup` reports as `EntryChanged`.
    #[tokio::test]
    async fn a_cleared_entry_has_no_lock_file_to_revalidate_against() {
        let (f, _input) = Fixture::with_input("main.py", b"body").await;

        let key = key("linux-64-abc");
        assert!(
            f.cache
                .open_lock_file_if_exists(&pkg("foo"), &key)
                .await
                .unwrap()
                .is_some(),
        );

        f.cache.clear_package(&pkg("foo")).unwrap();
        assert!(
            f.cache
                .open_lock_file_if_exists(&pkg("foo"), &key)
                .await
                .unwrap()
                .is_none(),
            "a cleared entry must report a missing lock rather than recreating it",
        );
    }

    /// A directory input whose mtime lands after the build started is dropped
    /// from the entry instead of being recorded, so nothing validates it any
    /// more. A regular file in the same position gets an unconfirmed
    /// fingerprint and costs one rebuild; the directory silently loses its
    /// validation and every later change to it is invisible.
    #[tokio::test]
    async fn a_directory_input_past_the_build_start_stays_validated() {
        let f = Fixture::new();
        let dir = f.source.join("assets");
        fs_err::create_dir_all(&dir).unwrap();
        let input = f.write("main.py", b"body");

        // The regular file predates the build, the directory does not.
        let build_started = mtime(&dir) - std::time::Duration::from_secs(10);
        set_modified(&input, build_started - std::time::Duration::from_secs(10));

        // Control: a store that starts after both mtimes records the
        // directory, and adding a file inside it then invalidates the entry.
        f.store(Vec::new(), vec![abs(input.clone()), abs(dir.clone())])
            .await;
        assert!(f.sidecar().input_files.contains_key(&abs(dir.clone())));
        fs_err::write(dir.join("control.txt"), b"x").unwrap();
        assert!(
            f.lookup().await.is_none(),
            "control: a recorded directory whose contents changed invalidates",
        );

        f.store_with_build_started(
            Vec::new(),
            vec![abs(input.clone()), abs(dir.clone())],
            build_started,
        )
        .await;
        assert!(
            f.sidecar().input_files.contains_key(&abs(dir.clone())),
            "an input the build read must stay recorded; dropping it means no \
             later change to it can ever invalidate the entry",
        );

        fs_err::write(dir.join("added.txt"), b"x").unwrap();
        assert!(
            f.lookup().await.is_none(),
            "changing a recorded input directory must invalidate the entry",
        );
    }

    /// The single key of the sidecar's state map.
    fn only_state_key(json: &serde_json::Value) -> String {
        let files = json["input_file_states"]["files"].as_object().unwrap();
        assert_eq!(files.len(), 1, "fixture stores exactly one input file");
        files.keys().next().unwrap().clone()
    }

    /// A sidecar can carry a file in `input_files` with no matching entry in
    /// the state map (a hand-edited or partially written entry). The artifact
    /// path verifies without a fallback cutoff, so nothing vouches for that
    /// file: it must miss rather than pass unvalidated.
    #[tokio::test]
    async fn a_state_map_missing_one_file_misses_instead_of_trusting_it() {
        let f = Fixture::new();
        let main = f.write("main.py", b"body");
        let other = f.write("other.py", b"other");
        f.store(
            vec![glob_group(&["**/*.py"])],
            vec![abs(main.clone()), abs(other.clone())],
        )
        .await;

        // Drop only `other.py`'s state; it stays listed in `input_files`.
        let mut json = f.sidecar_json();
        json["input_file_states"]["files"]
            .as_object_mut()
            .unwrap()
            .retain(|path, _| !path.ends_with("other.py"));
        f.write_sidecar_json(&json);
        let edited = fs_err::read(f.sidecar_path()).unwrap();

        match f.lookup_miss().await {
            CacheMissReason::InputFileUntracked { path } => {
                assert_eq!(
                    path.as_std_path(),
                    other,
                    "the file without a recorded state is the one that invalidates",
                );
            }
            reason => panic!("expected a miss on the stateless file, got {reason:?}"),
        }

        // Touching the file that *does* have a state runs the same partial map
        // through `verify` again. It must still miss on the stateless file,
        // and leave the entry byte-identical.
        touch(&main, 5);
        assert!(matches!(
            f.lookup_miss().await,
            CacheMissReason::InputFileUntracked { .. }
        ));
        assert_eq!(
            fs_err::read(f.sidecar_path()).unwrap(),
            edited,
            "a miss must not rewrite the entry it rejected",
        );
    }

    /// The same absolute path can reach a capture more than once (a variant
    /// file that is also a glob-matched input). It must be recorded once and
    /// keep validating normally.
    #[tokio::test]
    async fn an_input_path_listed_twice_is_recorded_once() {
        let f = Fixture::new();
        let input = f.write("main.py", b"body");
        f.store(
            vec![glob_group(&["**/*.py"])],
            vec![abs(input.clone()), abs(input.clone()), abs(input.clone())],
        )
        .await;

        let sidecar = f.sidecar();
        assert_eq!(
            sidecar.input_files.len(),
            1,
            "a duplicate must not be recorded twice"
        );
        assert_eq!(sidecar.input_file_states.iter().count(), 1);

        assert!(f.lookup().await.is_some(), "a deduplicated entry must hit");

        // The verify pass sees the duplicate too, through `input_files`.
        touch(&input, 5);
        assert!(
            f.lookup().await.is_some(),
            "a touched but unchanged duplicate must refresh, not miss",
        );
        assert_eq!(f.sidecar().input_files.len(), 1);
    }

    /// A legacy entry that misses must leave its recorded mtimes untouched;
    /// only the rebuild that follows may rewrite them.
    #[tokio::test]
    async fn a_legacy_sidecar_miss_does_not_rewrite_the_recorded_mtimes() {
        let (f, input) = Fixture::with_input("main.py", b"body").await;

        let mut json = f.sidecar_json();
        json.as_object_mut().unwrap().remove("input_file_states");
        f.write_sidecar_json(&json);
        let legacy_bytes = fs_err::read(f.sidecar_path()).unwrap();

        touch(&input, 7);
        assert!(matches!(
            f.lookup_miss().await,
            CacheMissReason::InputFileModified { .. }
        ));
        assert_eq!(
            fs_err::read(f.sidecar_path()).unwrap(),
            legacy_bytes,
            "a legacy miss must not rewrite the entry",
        );
    }

    /// An `MtimeOnly` state records no size, so a file that was unhashable at
    /// store time and is an ordinary file at probe time is validated on its
    /// mtime alone: a rewrite that restores the mtime is invisible even when
    /// the size changes. This is weaker than the fingerprint path (which
    /// compares the size first) and is recorded here so the gap is a
    /// conscious one.
    #[tokio::test]
    async fn an_mtime_only_state_accepts_a_size_change_that_restores_the_mtime() {
        let (f, input) = Fixture::with_input("main.py", b"body").await;

        // Emulate a file that could not be hashed when the entry was stored.
        let mut json = f.sidecar_json();
        let path_key = only_state_key(&json);
        let recorded_mtime = json["input_files"][&path_key].clone();
        json["input_file_states"]["files"][&path_key] =
            serde_json::json!({ "mtime_only": recorded_mtime });
        f.write_sidecar_json(&json);
        assert!(matches!(
            f.sidecar().input_file_states.get(&input),
            Some(InputFileState::MtimeOnly(_)),
        ));

        let original = mtime(&input);
        assert!(
            f.lookup().await.is_some(),
            "an untouched mtime-only file must hit",
        );

        fs_err::write(&input, b"a much longer body than before").unwrap();
        set_modified(&input, original);
        assert!(
            f.lookup().await.is_some(),
            "known limitation: mtime-only validation records no size, so a \
             rewrite that restores the mtime is not detected",
        );
        assert!(matches!(
            f.sidecar().input_file_states.get(&input),
            Some(InputFileState::MtimeOnly(_)),
        ));
    }

    /// The concurrent-store recheck is a byte compare-and-swap on the
    /// sidecar. Both halves (the refresh write and the plain check) must
    /// report "changed" for a byte image the entry never held; `lookup` maps
    /// that onto [`CacheMissReason::EntryChanged`].
    #[tokio::test]
    async fn a_byte_image_the_entry_never_held_fails_both_halves_of_the_recheck() {
        let (f, _input) = Fixture::with_input("main.py", b"body").await;
        let key = key("linux-64-abc");
        let stale_bytes = b"{}".to_vec();
        let sidecar = f.sidecar();
        let committed = fs_err::read(f.sidecar_path()).unwrap();

        assert!(matches!(
            f.cache
                .try_refresh_sidecar(&pkg("foo"), &key, &stale_bytes, &sidecar)
                .await
                .unwrap(),
            SidecarRefresh::Changed,
        ));
        assert!(
            !f.cache
                .sidecar_unchanged(&pkg("foo"), &key, &stale_bytes)
                .await
                .unwrap(),
        );
        assert_eq!(
            fs_err::read(f.sidecar_path()).unwrap(),
            committed,
            "a lost compare-and-swap must not write anything",
        );

        // A cleared entry reports the same, without resurrecting the entry.
        f.cache.clear_package(&pkg("foo")).unwrap();
        assert!(matches!(
            f.cache
                .try_refresh_sidecar(&pkg("foo"), &key, &committed, &sidecar)
                .await
                .unwrap(),
            SidecarRefresh::Changed,
        ));
        assert!(
            !f.sidecar_path().exists(),
            "a refresh must never recreate a cleared entry",
        );
    }

    #[cfg(unix)]
    fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    /// Input files are stat'ed and hashed through the link, so an edit to the
    /// target invalidates the entry even when the link itself is untouched
    /// and the size is unchanged.
    #[tokio::test]
    async fn editing_the_target_of_a_symlinked_input_invalidates_the_entry() {
        let f = Fixture::new();
        // The target lives outside the source dir so the glob walk only ever
        // sees the link.
        let target = f.source.parent().unwrap().join("target.py");
        fs_err::write(&target, b"old").unwrap();
        let link = f.source.join("main.py");
        if let Err(err) = symlink_file(&target, &link) {
            // Creating symlinks needs a privilege or developer mode on Windows.
            eprintln!("skipping: cannot create symlinks here ({err})");
            return;
        }

        f.store(vec![glob_group(&["**/*.py"])], vec![abs(link.clone())])
            .await;
        assert!(f.lookup().await.is_some(), "an untouched link must hit");

        let base = mtime(&link);
        fs_err::write(&target, b"new").unwrap();
        set_modified(&target, base + std::time::Duration::from_secs(1));
        assert!(
            f.lookup().await.is_none(),
            "a same-size edit to the link target must invalidate the entry",
        );
    }

    /// The hard-link counterpart of the symlink case, which needs no
    /// privileges: an input edited through a second name for the same file
    /// must invalidate the entry.
    #[tokio::test]
    async fn editing_a_hard_linked_input_through_its_other_name_invalidates_the_entry() {
        let f = Fixture::new();
        // The second name lives outside the source dir so the glob walk only
        // ever sees the recorded input.
        let other_name = f.source.parent().unwrap().join("other-name.py");
        let input = f.write("main.py", b"old");
        if let Err(err) = fs_err::hard_link(&input, &other_name) {
            eprintln!("skipping: cannot create hard links here ({err})");
            return;
        }

        f.store(vec![glob_group(&["**/*.py"])], vec![abs(input.clone())])
            .await;
        assert!(f.lookup().await.is_some(), "an untouched input must hit");

        let base = mtime(&input);
        fs_err::write(&other_name, b"new").unwrap();
        set_modified(&other_name, base + std::time::Duration::from_secs(1));
        assert!(
            f.lookup().await.is_none(),
            "a same-size edit through the other name must invalidate the entry",
        );
    }
}

#[cfg(test)]
mod cache_key_tests {
    //! Every input to `compute_artifact_cache_key` must contribute to the
    //! key: equivalent inputs produce equal keys, any single-field change
    //! produces a different key. These tests are the guardrail against
    //! silent cache collisions when the hash input set evolves.
    use std::{collections::BTreeMap, str::FromStr, sync::Arc};

    use pixi_record::{
        FullSourceRecordData, PinnedPathSpec, PinnedSourceSpec, SourceRecordData,
        UnresolvedPixiRecord, UnresolvedSourceRecord,
    };
    use rattler_conda_types::{PackageName, PackageRecord, Platform, RepoDataRecord};
    use rattler_digest::{Sha256Hash, parse_digest_from_hex};
    use typed_path::Utf8TypedPathBuf;

    use super::compute_artifact_cache_key;

    fn record(name: &str) -> UnresolvedSourceRecord {
        let mut pr = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        UnresolvedSourceRecord {
            data: SourceRecordData::Full(FullSourceRecordData {
                package_record: pr,
                sources: BTreeMap::new(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: Utf8TypedPathBuf::from(format!("./{name}")),
            }),
            build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: String::new(),
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        }
    }

    fn binary_dep(name: &str, url: &str, sha: &str) -> UnresolvedPixiRecord {
        let mut pr = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        pr.sha256 = parse_digest_from_hex::<rattler_digest::Sha256>(sha);
        let repo = RepoDataRecord {
            package_record: pr,
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                &format!("{name}-1.0.0-h0.conda"),
            )
            .unwrap(),
            url: url::Url::parse(url).unwrap(),
            channel: None,
        };
        UnresolvedPixiRecord::Binary(Arc::new(repo))
    }

    fn sha(byte: u8) -> Sha256Hash {
        let mut out = [0u8; 32];
        out[0] = byte;
        Sha256Hash::from(out)
    }

    fn key_for(
        r: &UnresolvedSourceRecord,
        backend_id: &str,
        extra_build_sha: &[Sha256Hash],
    ) -> String {
        compute_artifact_cache_key(
            r,
            Platform::Linux64,
            Platform::Linux64,
            backend_id,
            extra_build_sha,
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string()
    }

    #[test]
    fn identical_inputs_produce_equal_keys() {
        let a = record("foo");
        let b = record("foo");
        assert_eq!(
            key_for(&a, "backend-v1", &[]),
            key_for(&b, "backend-v1", &[])
        );
    }

    #[test]
    fn package_name_matters() {
        let a = record("foo");
        let b = record("bar");
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn manifest_source_matters() {
        let a = record("foo");
        let mut b = record("foo");
        b.manifest_source = PinnedSourceSpec::Path(PinnedPathSpec {
            path: Utf8TypedPathBuf::from("./foo-alt"),
        });
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn variants_matter() {
        let a = record("foo");
        let mut b = record("foo");
        b.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.12".to_string()),
        );
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn variant_value_change_matters() {
        let mut a = record("foo");
        a.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.11".to_string()),
        );
        let mut b = record("foo");
        b.variants.insert(
            "python".into(),
            pixi_record::VariantValue::from("3.12".to_string()),
        );
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn build_platform_matters() {
        let r = record("foo");
        let k1 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::OsxArm64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn host_platform_matters() {
        let r = record("foo");
        let k1 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::OsxArm64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn backend_identifier_matters() {
        let r = record("foo");
        assert_ne!(
            key_for(&r, "backend-v1", &[]),
            key_for(&r, "backend-v2", &[])
        );
    }

    #[test]
    fn adding_a_binary_dep_changes_the_key() {
        let a = record("foo");
        let mut b = record("foo");
        b.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn binary_dep_url_matters() {
        let mut a = record("foo");
        a.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        let mut b = record("foo");
        b.build_packages.push(binary_dep(
            "numpy",
            "https://different.mirror/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn binary_dep_sha256_matters() {
        let mut a = record("foo");
        a.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        let mut b = record("foo");
        b.build_packages.push(binary_dep(
            "numpy",
            "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.0-h0.conda",
            "bb00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn build_source_dep_sha256_matters() {
        let r = record("foo");
        assert_ne!(
            key_for(&r, "b", &[sha(0xaa)]),
            key_for(&r, "b", &[sha(0xbb)]),
        );
    }

    #[test]
    fn host_source_dep_sha256_matters() {
        let r = record("foo");
        let k1 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xaa)],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let k2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xbb)],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        assert_ne!(k1, k2);
    }

    #[test]
    fn source_dep_bucket_matters() {
        // A source dep with identical sha256 placed in the build vs host
        // bucket must hash to different keys. The bucket determines
        // which prefix it installs into and thus which compat map the
        // run-dep resolution sees.
        let r = record("foo");
        let build_only = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[sha(0xaa)],
            &[],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        let host_only = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[sha(0xaa)],
            &Default::default(),
            None,
            None,
        )
        .to_string();
        assert_ne!(build_only, host_only);
    }

    #[test]
    fn source_dep_order_within_a_bucket_matters() {
        // Two source deps with different sha256s in the same bucket:
        // swapping order produces a different key.
        let r = record("foo");
        assert_ne!(
            key_for(&r, "b", &[sha(0xaa), sha(0xbb)]),
            key_for(&r, "b", &[sha(0xbb), sha(0xaa)]),
        );
    }

    #[test]
    fn binary_dep_bucket_matters() {
        // Same binary dep placed in build_packages vs host_packages now
        // produces different keys (bucket-tagged). This is the fix for
        // the earlier collision bug.
        let mut a = record("foo");
        a.build_packages.push(binary_dep(
            "numpy",
            "https://x.test/numpy.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        let mut b = record("foo");
        b.host_packages.push(binary_dep(
            "numpy",
            "https://x.test/numpy.conda",
            "aa00000000000000000000000000000000000000000000000000000000000000",
        ));
        assert_ne!(key_for(&a, "b", &[]), key_for(&b, "b", &[]));
    }

    #[test]
    fn cache_key_is_deterministic_across_runs() {
        // Two independently-built records produce identical keys; the
        // hasher's state is reset per call.
        let r1 = record("foo");
        let r2 = record("foo");
        assert_eq!(
            key_for(&r1, "backend-v1", &[sha(0x01), sha(0x02)]),
            key_for(&r2, "backend-v1", &[sha(0x01), sha(0x02)]),
        );
    }

    #[test]
    fn cache_key_changes_with_host_platform() {
        // Host platform is hashed into the key but not displayed in
        // it (short-path policy). Two keys that differ only by host
        // platform must therefore be distinct.
        let r = record("foo");
        let linux = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        let osx_arm = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::OsxArm64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        assert_ne!(linux, osx_arm);
    }

    #[test]
    fn build_string_prefix_matters() {
        let r = record("foo");
        let bare = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        let prefixed = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &crate::ProjectModelOverrides {
                build_string_prefix: Some("foobar".to_string()),
                build_number: None,
            },
            None,
            None,
        );
        assert_ne!(bare, prefixed);
    }

    #[test]
    fn build_number_matters() {
        let r = record("foo");
        let bare = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            None,
            None,
        );
        let numbered = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &crate::ProjectModelOverrides {
                build_string_prefix: None,
                build_number: Some(42),
            },
            None,
            None,
        );
        assert_ne!(bare, numbered);
    }

    #[test]
    fn archive_type_matters() {
        use pixi_build_types::procedures::conda_build_v1::CondaPackageFormat;
        use rattler_conda_types::package::CondaArchiveType;
        let r = record("foo");
        let conda = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            Some(CondaPackageFormat {
                archive_type: CondaArchiveType::Conda,
                compression_level: Default::default(),
            }),
            None,
        );
        let tar_bz2 = compute_artifact_cache_key(
            &r,
            Platform::Linux64,
            Platform::Linux64,
            "b",
            &[],
            &[],
            &Default::default(),
            Some(CondaPackageFormat {
                archive_type: CondaArchiveType::TarBz2,
                compression_level: Default::default(),
            }),
            None,
        );
        assert_ne!(conda, tar_bz2);
    }

    #[test]
    fn compression_level_matters() {
        use pixi_build_types::procedures::conda_build_v1::{
            CondaCompressionLevel, CondaPackageFormat, NamedCompressionLevel,
        };
        use rattler_conda_types::package::CondaArchiveType;
        let pf = |level: CondaCompressionLevel| CondaPackageFormat {
            archive_type: CondaArchiveType::Conda,
            compression_level: level,
        };
        let r = record("foo");
        let key = |level: CondaCompressionLevel| {
            compute_artifact_cache_key(
                &r,
                Platform::Linux64,
                Platform::Linux64,
                "b",
                &[],
                &[],
                &Default::default(),
                Some(pf(level)),
                None,
            )
        };
        let default_level = key(CondaCompressionLevel::Named(NamedCompressionLevel::Default));
        let max_level = key(CondaCompressionLevel::Named(NamedCompressionLevel::Highest));
        let numeric_level = key(CondaCompressionLevel::Numeric(5));
        assert_ne!(default_level, max_level);
        assert_ne!(default_level, numeric_level);
        assert_ne!(max_level, numeric_level);
    }
}
