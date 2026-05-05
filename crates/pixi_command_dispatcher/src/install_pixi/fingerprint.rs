//! [`EnvironmentFingerprint`]: a content fingerprint of every record
//! installed into a pixi prefix.
//!
//! Collapses an install's `Vec<RepoDataRecord>` down to a small
//! deterministic string by hashing each record's name + sha256.
//! Suitable as a cache key for downstream work that depends on
//! "what is currently in this prefix" — most notably the activation
//! cache, which can short-circuit when the prefix's contents
//! haven't changed since the last successful activation.
//!
//! The fingerprint persists alongside the prefix in a small
//! standalone marker file ([`EnvironmentFingerprint::MARKER_FILENAME`],
//! written under `<env_dir>/conda-meta/`). Methods
//! [`EnvironmentFingerprint::read`] and [`EnvironmentFingerprint::write`]
//! manage that file directly so callers don't need to know about its
//! location or format.

use std::{fmt, hash::Hasher, path::Path};

use rattler_conda_types::RepoDataRecord;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

/// Content fingerprint of every record installed into a prefix.
///
/// Computed from the installed records' per-record sha256s
/// (binaries from the lockfile + built source-build artifacts whose
/// sha256 is recorded in the artifact-cache sidecar). The names are
/// folded in too so two distinct packages that happen to share a
/// sha256 don't collide, and inputs are sorted so insertion order
/// doesn't perturb the result.
///
/// `Eq` / `Hash` compare strings, so two `EnvironmentFingerprint`s
/// can be checked for equality without rehashing.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentFingerprint(String);

impl EnvironmentFingerprint {
    /// Compute the fingerprint over an iterable of records.
    ///
    /// Cheap: just iterate, sort by name, and feed bytes into xxh3.
    /// Every record already carries its sha256, so we never re-read
    /// any files on disk.
    pub fn compute<'a>(records: impl IntoIterator<Item = &'a RepoDataRecord>) -> Self {
        let mut inputs: Vec<(&str, Option<&[u8]>)> = records
            .into_iter()
            .map(|r| {
                (
                    r.package_record.name.as_normalized(),
                    r.package_record.sha256.as_ref().map(|h| h.as_slice()),
                )
            })
            .collect();
        // Sort by name so the fingerprint is independent of the
        // order in which the install pipeline emitted records.
        inputs.sort_by(|a, b| a.0.cmp(b.0));
        let mut hasher = Xxh3::new();
        for (name, sha) in &inputs {
            std::hash::Hash::hash(name, &mut hasher);
            match sha {
                Some(bytes) => std::hash::Hash::hash(*bytes, &mut hasher),
                None => std::hash::Hash::hash(&0u8, &mut hasher),
            }
        }
        EnvironmentFingerprint(format!("{:016x}", hasher.finish()))
    }

    /// Borrow the underlying hex digest. Useful for cache-key
    /// composition that wants to fold the fingerprint into a larger
    /// hash without consuming the wrapper.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reconstruct from a previously-stored string. Use when reading
    /// the fingerprint back from external state; computation should
    /// always go through [`Self::compute`].
    pub fn from_string(s: String) -> Self {
        EnvironmentFingerprint(s)
    }

    /// Filename of the marker written under `<env_dir>/conda-meta/`.
    /// Hidden so it doesn't show up alongside conda's own
    /// per-package metadata files.
    const MARKER_FILENAME: &'static str = ".pixi-environment-fingerprint";

    /// Resolve the marker path for a prefix's environment directory.
    fn marker_path(env_dir: &Path) -> std::path::PathBuf {
        env_dir.join("conda-meta").join(Self::MARKER_FILENAME)
    }

    /// Read the cached fingerprint for an environment, if one was
    /// written by a previous successful install.
    ///
    /// Returns `None` when the marker is absent (no install has
    /// run yet, or the prefix was wiped) or unreadable. Activation
    /// caching uses this to decide whether the prefix's content
    /// is still authoritative; a missing marker just means we run
    /// activation fresh, never produces a stale hit.
    pub fn read(env_dir: &Path) -> Option<Self> {
        let bytes = fs_err::read(Self::marker_path(env_dir)).ok()?;
        let trimmed = std::str::from_utf8(&bytes).ok()?.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(EnvironmentFingerprint(trimmed.to_string()))
    }

    /// Persist the fingerprint to the prefix's marker file. Atomic
    /// (write-temp + rename) so a concurrent reader never sees a
    /// truncated file. Returns the io error verbatim; callers that
    /// only need best-effort persistence can ignore it.
    pub fn write(&self, env_dir: &Path) -> std::io::Result<()> {
        let dest = Self::marker_path(env_dir);
        if let Some(parent) = dest.parent() {
            fs_err::create_dir_all(parent)?;
        }
        let tmp = dest.with_extension("tmp");
        fs_err::write(&tmp, self.0.as_bytes())?;
        fs_err::rename(&tmp, &dest)
    }
}

impl fmt::Display for EnvironmentFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
