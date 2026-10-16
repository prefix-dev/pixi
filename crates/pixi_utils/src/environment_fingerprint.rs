//! [`EnvironmentFingerprint`]: fingerprint of every record installed into a
//! pixi prefix, derived from its name and strongest available checksum
//! (SHA-256, then MD5), or package identity when neither is available. Used as
//! a cache key by downstream consumers like the activation cache.
//!
//! Persisted under the install lock managed by
//! [`crate::EnvironmentLock`]; [`EnvironmentFingerprint::read`] is a
//! lock-free peek for read-only consumers.

use std::{
    fmt,
    hash::{Hash, Hasher},
    path::Path,
};

use rattler_conda_types::RepoDataRecord;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

use crate::environment_lock::{FINGERPRINT_WIDTH, marker_path};

/// Fingerprint of every record installed into a prefix.
/// Folds each record's name and checksum (or package identity), sorted by
/// name so the result is order-independent.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentFingerprint(String);

impl EnvironmentFingerprint {
    /// Compute the fingerprint using existing record metadata (no file I/O).
    pub fn compute<'a>(records: impl IntoIterator<Item = &'a RepoDataRecord>) -> Self {
        let mut inputs: Vec<&RepoDataRecord> = records.into_iter().collect();
        inputs.sort_by(|a, b| {
            a.package_record
                .name
                .as_normalized()
                .cmp(b.package_record.name.as_normalized())
        });
        let mut hasher = Xxh3::new();
        for record in inputs {
            // Take the package record
            let package = &record.package_record;
            package.name.as_normalized().hash(&mut hasher);

            match (&package.sha256, &package.md5) {
                // For sha, we just use the sha
                (Some(sha256), _) => sha256.as_slice().hash(&mut hasher),
                // Otherwise lets take the md5
                (None, Some(md5)) => {
                    md5.as_slice().hash(&mut hasher);
                }
                // `describe_same_content` in rattler does the same
                (None, None) => {
                    "record".hash(&mut hasher);
                    record.url.as_str().hash(&mut hasher);
                    package.version.hash(&mut hasher);
                    package.build.hash(&mut hasher);
                    package.build_number.hash(&mut hasher);
                    package.subdir.hash(&mut hasher);
                    package.size.hash(&mut hasher);
                }
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

#[cfg(test)]
mod tests {
    use rattler_conda_types::{PackageRecord, package::DistArchiveIdentifier};
    use url::Url;

    use super::*;

    fn record(name: &str, version: &str) -> RepoDataRecord {
        let filename = format!("{name}-{version}-0.tar.bz2");
        let mut package_record = PackageRecord::new(
            name.parse().unwrap(),
            version
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "0".into(),
        );
        package_record.subdir = "noarch".into();
        RepoDataRecord {
            package_record,
            identifier: DistArchiveIdentifier::try_from_filename(&filename).unwrap(),
            url: Url::parse(&format!("https://example.com/noarch/{filename}")).unwrap(),
            channel: None,
        }
    }

    #[test]
    fn md5_only_content_change_invalidates_fingerprint() {
        let mut old = record("foo", "1.0.0");
        old.package_record.md5 = Some([1; 16].into());
        let mut new = old.clone();
        new.package_record.md5 = Some([2; 16].into());

        assert_ne!(
            EnvironmentFingerprint::compute([&old]),
            EnvironmentFingerprint::compute([&new]),
            "different MD5 content must not share an installation fingerprint",
        );
    }

    #[test]
    fn checksum_free_package_update_invalidates_fingerprint() {
        let old = record("foo", "1.0.0");
        let new = record("foo", "2.0.0");

        assert_ne!(
            EnvironmentFingerprint::compute([&old]),
            EnvironmentFingerprint::compute([&new]),
            "different package identities must not share an installation fingerprint",
        );
    }

    #[test]
    fn sha256_takes_precedence_over_md5() {
        let mut old = record("foo", "1.0.0");
        old.package_record.sha256 = Some([1; 32].into());
        old.package_record.md5 = Some([1; 16].into());
        let mut new = old.clone();
        new.package_record.md5 = Some([2; 16].into());

        assert_eq!(
            EnvironmentFingerprint::compute([&old]),
            EnvironmentFingerprint::compute([&new]),
        );

        new.package_record.sha256 = Some([2; 32].into());
        assert_ne!(
            EnvironmentFingerprint::compute([&old]),
            EnvironmentFingerprint::compute([&new]),
        );
    }
}
