//! On-disk cache for [`LegacySourceEnv`] results, scoped to a single
//! workspace under `.pixi/legacy-source-env/`.
//!
//! Each entry is a JSON file named after a stable hash of the inputs
//! that affect correctness of the underlying
//! [`ResolveSourcePackageKey`](pixi_command_dispatcher::keys::ResolveSourcePackageKey)
//! dispatch. Cache files round-trip the full transitive
//! `build_packages` / `host_packages` tree, so a cache hit avoids both
//! the build-backend metadata fetch and any nested solves.
//!
//! The cache is best-effort: read errors, parse errors, and write
//! errors are logged at trace level and treated as cache misses.
//! Correctness is preserved by recomputing on miss, and re-running
//! verification (which would re-validate the result against the
//! backend's declared specs) is what catches genuine drift.
//!
//! The file format embeds a `schema_version` field; bumps invalidate
//! older entries (they fail to parse and we recompute).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use pixi_record::{
    PinnedBuildSourceSpec, PinnedSourceSpec, SourceRecordData, UnresolvedPixiRecord,
    UnresolvedSourceRecord, VariantValue,
};
use rattler_conda_types::RepoDataRecord;
use serde::{Deserialize, Serialize};

use super::key::LegacySourceEnv;

/// Bumped on any incompatible change to the on-disk format. Older
/// entries fail to parse and are treated as cache misses.
const CACHE_SCHEMA_VERSION: u32 = 1;

/// Cache directory, relative to the workspace root.
const CACHE_DIRNAME: &str = ".pixi/legacy-source-env";

/// On-disk shape for a cached [`LegacySourceEnv`].
#[derive(Serialize, Deserialize)]
struct CachedFile {
    schema_version: u32,
    build_packages: Vec<WireRecord>,
    host_packages: Vec<WireRecord>,
}

/// Wire variant of [`UnresolvedPixiRecord`]. We can't reuse the
/// existing serde derives directly because
/// [`SourceRecord`](pixi_record::SourceRecord) marks
/// `build_packages` / `host_packages` as `#[serde(skip)]` (the
/// canonical lockfile encodes those via index references on a
/// separate side table). Our cache needs the full tree per file, so
/// we mirror the shape with the slices included.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireRecord {
    Binary { record: Box<RepoDataRecord> },
    Source { record: Box<WireSource> },
}

#[derive(Serialize, Deserialize)]
struct WireSource {
    data: SourceRecordData,
    manifest_source: PinnedSourceSpec,
    build_source: Option<PinnedBuildSourceSpec>,
    variants: BTreeMap<String, VariantValue>,
    identifier_hash: Option<String>,
    build_packages: Vec<WireRecord>,
    host_packages: Vec<WireRecord>,
}

fn to_wire(record: &UnresolvedPixiRecord) -> WireRecord {
    match record {
        UnresolvedPixiRecord::Binary(b) => WireRecord::Binary {
            record: Box::new((**b).clone()),
        },
        UnresolvedPixiRecord::Source(s) => WireRecord::Source {
            record: Box::new(to_wire_source(s)),
        },
    }
}

fn to_wire_source(record: &UnresolvedSourceRecord) -> WireSource {
    WireSource {
        data: record.data.clone(),
        manifest_source: record.manifest_source.clone(),
        build_source: record.build_source.clone(),
        variants: record.variants.clone(),
        identifier_hash: record.identifier_hash.clone(),
        build_packages: record.build_packages.iter().map(to_wire).collect(),
        host_packages: record.host_packages.iter().map(to_wire).collect(),
    }
}

fn from_wire(wire: WireRecord) -> UnresolvedPixiRecord {
    match wire {
        WireRecord::Binary { record } => UnresolvedPixiRecord::Binary(Arc::new(*record)),
        WireRecord::Source { record } => {
            UnresolvedPixiRecord::Source(Arc::new(from_wire_source(*record)))
        }
    }
}

fn from_wire_source(wire: WireSource) -> UnresolvedSourceRecord {
    UnresolvedSourceRecord {
        data: wire.data,
        manifest_source: wire.manifest_source,
        build_source: wire.build_source,
        variants: wire.variants,
        identifier_hash: wire.identifier_hash,
        build_packages: wire.build_packages.into_iter().map(from_wire).collect(),
        host_packages: wire.host_packages.into_iter().map(from_wire).collect(),
    }
}

/// Path to the cache file for a given workspace + key hash.
pub(super) fn cache_file_path(workspace_root: &Path, key_hash: u64) -> PathBuf {
    workspace_root
        .join(CACHE_DIRNAME)
        .join(format!("{key_hash:016x}.json"))
}

/// Try to load a cached [`LegacySourceEnv`] from disk. Returns `None`
/// on any failure (missing file, IO error, malformed JSON, schema
/// mismatch); callers treat that as a cache miss.
pub(super) fn load(path: &Path) -> Option<LegacySourceEnv> {
    let content = match fs_err::read_to_string(path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::trace!(?path, %err, "legacy-source-env cache read failed");
            return None;
        }
    };
    let cached: CachedFile = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(err) => {
            tracing::trace!(?path, %err, "legacy-source-env cache parse failed");
            return None;
        }
    };
    if cached.schema_version != CACHE_SCHEMA_VERSION {
        tracing::trace!(
            ?path,
            file_version = cached.schema_version,
            current_version = CACHE_SCHEMA_VERSION,
            "legacy-source-env cache schema version mismatch; treating as miss"
        );
        return None;
    }
    Some(LegacySourceEnv {
        build_packages: cached.build_packages.into_iter().map(from_wire).collect(),
        host_packages: cached.host_packages.into_iter().map(from_wire).collect(),
    })
}

/// Best-effort write of `env` to `path`. Errors are logged and
/// swallowed: a failed write turns the next read into a miss, which
/// is correct behavior. The write is atomic via temp + rename so a
/// concurrent reader never sees a partial file.
pub(super) fn store(path: &Path, env: &LegacySourceEnv) {
    if let Err(err) = store_inner(path, env) {
        tracing::trace!(?path, %err, "legacy-source-env cache write failed");
    }
}

fn store_inner(path: &Path, env: &LegacySourceEnv) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let cached = CachedFile {
        schema_version: CACHE_SCHEMA_VERSION,
        build_packages: env.build_packages.iter().map(to_wire).collect(),
        host_packages: env.host_packages.iter().map(to_wire).collect(),
    };
    let json = serde_json::to_vec(&cached).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs_err::write(&tmp, json)?;
    fs_err::rename(&tmp, path)?;
    Ok(())
}
