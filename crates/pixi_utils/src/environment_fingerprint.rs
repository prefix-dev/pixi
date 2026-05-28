//! [`EnvironmentFingerprint`]: content fingerprint of every record
//! installed into a pixi prefix, derived from each record's
//! `name + sha256`. Used as a cache key by downstream consumers like
//! the activation cache.
//!
//! Persisted under the install lock managed by
//! [`crate::EnvironmentLock`]; [`EnvironmentFingerprint::read`] is a
//! lock-free peek for read-only consumers.

use std::{fmt, hash::Hasher, path::Path};

use rattler_conda_types::RepoDataRecord;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

use crate::environment_lock::{FINGERPRINT_WIDTH, marker_path};

/// Content fingerprint of every record installed into a prefix.
/// Folds `name + sha256` from each record, sorted by name so the
/// result is order-independent.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentFingerprint(String);

impl EnvironmentFingerprint {
    /// Compute the fingerprint over an iterable of records. Cheap;
    /// uses each record's existing sha256 (no file I/O).
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
        inputs.sort_by(|a, b| a.0.cmp(b.0));
        let mut hasher = Xxh3::new();
        for (name, sha) in &inputs {
            std::hash::Hash::hash(name, &mut hasher);
            match sha {
                Some(bytes) => std::hash::Hash::hash(*bytes, &mut hasher),
                None => std::hash::Hash::hash(&0u8, &mut hasher),
            }
        }
        let s = format!("{:016x}", hasher.finish());
        debug_assert_eq!(s.len(), FINGERPRINT_WIDTH);
        EnvironmentFingerprint(s)
    }

    /// The underlying hex digest, for cache-key composition.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reconstruct from a previously-stored string.
    pub fn from_string(s: String) -> Self {
        EnvironmentFingerprint(s)
    }

    /// Lock-free read of the on-disk fingerprint, for best-effort
    /// consumers like the activation cache. Returns `None` unless a
    /// completed install recorded a valid fingerprint (an in-progress
    /// marker reads as `None`).
    pub fn read(prefix_dir: &Path) -> Option<Self> {
        let bytes = fs_err::read(marker_path(prefix_dir)).ok()?;
        let head = bytes.get(..FINGERPRINT_WIDTH)?;
        if !head.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let s = std::str::from_utf8(head).ok()?;
        Some(EnvironmentFingerprint(s.to_string()))
    }

    /// Fixed-width bytes for the on-disk format used by
    /// [`crate::EnvironmentLock`]. Crate-internal: callers pass the
    /// typed value to `matches` / `finish` directly.
    pub(crate) fn as_bytes(&self) -> [u8; FINGERPRINT_WIDTH] {
        debug_assert_eq!(self.0.len(), FINGERPRINT_WIDTH);
        let mut out = [0u8; FINGERPRINT_WIDTH];
        out.copy_from_slice(self.0.as_bytes());
        out
    }
}

impl fmt::Display for EnvironmentFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
