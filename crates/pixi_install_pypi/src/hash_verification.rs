//! Verification of locked PyPI artifacts against the hashes in the lock file.
//!
//! The lock file pins registry artifacts (wheels and sdists) to a digest.
//! uv only checks digests when it is given a [`HashStrategy`] that demands it.
//! This module derives that strategy from the locked records.
//! uv enforces it at the two points where an artifact can enter the environment:
//!
//! - **Fetching**: an artifact missing from uv's cache is downloaded or read from disk.
//!   uv hashes the bytes and stores the digest alongside the cache entry.
//!   The install fails when the digest does not match the locked one
//!   (`Preparer` / `DistributionDatabase`).
//! - **Cache reuse**: an artifact fetched by an earlier run is reused without network access.
//!   There are no bytes to hash, so the digest stored at fetch time must satisfy the locked one.
//!   On a mismatch the cache entry is ignored and the artifact goes through fetching,
//!   and is thereby verified, again (`RegistryWheelIndex` / `BuiltWheelIndex`).
//!
//! Coverage follows what the lock file pins.
//! Today pixi records digests for registry distributions only.
//! Direct URL, path, git, and directory dependencies are locked without a hash.
//! They therefore install unverified.
//! Should lock generation ever pin direct URL or path archives, the keying already covers them.

use std::sync::Arc;

use pixi_uv_conversions::to_uv_hash_digests;
use rattler_lock::PackageHashes;
use rustc_hash::FxHashMap;
use uv_distribution_types::{Dist, DistributionMetadata, SourceDist, VersionId};
use uv_pypi_types::{HashAlgorithm, HashDigest};
use uv_types::HashStrategy;

use crate::plan::RequiredDists;

/// The expected digests of the locked PyPI artifacts, keyed the way uv identifies distributions.
///
/// Registry distributions are keyed by name and version.
/// Direct URL distributions are keyed by their URL.
/// Both keys come from [`DistributionMetadata::version_id`] on the exact [`Dist`] values that
/// are later installed, so lookups inside uv are guaranteed to hit.
#[derive(Debug)]
pub struct LockedDistHashes {
    hashes: FxHashMap<VersionId, Vec<HashDigest>>,
}

impl LockedDistHashes {
    /// Collect the locked digests for every required distribution that uv is able to hash.
    /// Distributions the lock file cannot pin to a digest are simply absent.
    /// Git and directory records are skipped even if a foreign or hand-edited lock file
    /// carries a digest for them.
    /// uv hard-fails on hash policies for sources it cannot hash
    /// (`HashesNotSupportedGit` / `HashesNotSupportedSourceTree`).
    pub fn from_required_dists(required_dists: &RequiredDists) -> Self {
        let hashes = required_dists
            .values()
            .filter_map(|data| {
                if !supports_hash_verification(&data.dist) {
                    return None;
                }
                let digests = verification_digests(data.record.hash.as_ref()?);
                Some((data.dist.version_id(), digests))
            })
            .collect();
        Self { hashes }
    }

    /// Turn the locked digests into the [`HashStrategy`] handed to uv.
    ///
    /// This uses [`HashStrategy::Verify`] rather than [`HashStrategy::Require`].
    /// Artifacts with a locked digest must match it.
    /// Artifacts without one install unverified, mirroring what the lock file is able to pin.
    /// The strategy defends against a tampered artifact (registry, mirror, or transport).
    /// It does not defend against a tampered lock file: whoever can edit it can drop the digest.
    pub fn into_verify_strategy(self) -> HashStrategy {
        HashStrategy::Verify(Arc::new(self.hashes))
    }
}

/// Whether uv can hash this distribution at all.
/// Git checkouts and source trees have no archive to digest.
/// uv hard-errors when a hash policy demands validation for them, so they must never get one.
fn supports_hash_verification(dist: &Dist) -> bool {
    !matches!(
        dist,
        Dist::Source(SourceDist::Git(_) | SourceDist::Directory(_))
    )
}

/// The digests an artifact is verified against.
///
/// When the lock file pins both an md5 and a sha256, only the sha256 is used.
/// uv's registry hash policy accepts any matching digest.
/// Including the md5 would let an md5 collision bypass the pinned sha256.
fn verification_digests(hash: &PackageHashes) -> Vec<HashDigest> {
    let mut digests = to_uv_hash_digests(hash);
    if digests
        .iter()
        .any(|digest| digest.algorithm == HashAlgorithm::Sha256)
    {
        digests.retain(|digest| digest.algorithm == HashAlgorithm::Sha256);
    }
    digests
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
    use rattler_lock::{PypiDistributionData, UrlOrPath};
    use uv_distribution_types::HashPolicy;

    use super::*;
    use crate::{InstallablePypiRecord, ManifestData};

    const SHA256_HEX: &str = "5e809d755e8619cb680b5d742cdd911390a377a1cc2e4a0e2b1c7a7cbfb957ff";
    const MD5_HEX: &str = "ad659d0a2b3e47e38d829aa8cad2d610";

    fn record(
        name: &str,
        location: UrlOrPath,
        hash: Option<PackageHashes>,
    ) -> InstallablePypiRecord {
        let version = pep440_rs::Version::from_str("1.0.0").unwrap();
        InstallablePypiRecord::new(
            &PypiDistributionData {
                name: name.parse().unwrap(),
                version: version.clone(),
                location: location.into(),
                hash,
                index_url: None,
                requires_dist: vec![],
                requires_python: None,
            },
            ManifestData { editable: false },
            version,
        )
    }

    fn sha256_hash() -> PackageHashes {
        PackageHashes::Sha256(parse_digest_from_hex::<Sha256>(SHA256_HEX).unwrap())
    }

    fn md5_sha256_hash() -> PackageHashes {
        PackageHashes::Md5Sha256(
            parse_digest_from_hex::<Md5>(MD5_HEX).unwrap(),
            parse_digest_from_hex::<Sha256>(SHA256_HEX).unwrap(),
        )
    }

    fn strategy_for(records: &[InstallablePypiRecord]) -> (HashStrategy, RequiredDists) {
        // Relative path records resolve against the lock file directory, which is absolute.
        let lock_file_dir = std::env::current_dir().unwrap();
        let required = RequiredDists::from_packages(records.iter(), &lock_file_dir).unwrap();
        let strategy = LockedDistHashes::from_required_dists(&required).into_verify_strategy();
        (strategy, required)
    }

    #[test]
    fn registry_wheel_with_locked_hash_is_verified() {
        let url = "https://files.pythonhosted.org/packages/foo-1.0.0-py3-none-any.whl"
            .parse()
            .unwrap();
        let records = [record("foo", UrlOrPath::Url(url), Some(sha256_hash()))];
        let (strategy, required) = strategy_for(&records);

        let dist = &required.values().next().unwrap().dist;
        let HashPolicy::Any(digests) = strategy.get(dist) else {
            panic!("expected the locked registry wheel to be validated");
        };
        assert_eq!(digests.len(), 1);
        assert_eq!(digests[0].algorithm, HashAlgorithm::Sha256);
        assert_eq!(digests[0].digest.as_ref(), SHA256_HEX);

        // The cache index looks distributions up by name and version.
        let name = uv_normalize::PackageName::from_str("foo").unwrap();
        let version = uv_pep440::Version::from_str("1.0.0").unwrap();
        assert!(matches!(
            strategy.get_package(&name, &version),
            HashPolicy::Any(_)
        ));
    }

    #[test]
    fn md5_is_dropped_when_sha256_is_locked() {
        // uv's registry policy accepts any matching digest.
        // Keeping the md5 around would let an md5 collision bypass the pinned sha256.
        let url = "https://files.pythonhosted.org/packages/foo-1.0.0-py3-none-any.whl"
            .parse()
            .unwrap();
        let records = [record("foo", UrlOrPath::Url(url), Some(md5_sha256_hash()))];
        let (strategy, required) = strategy_for(&records);

        let dist = &required.values().next().unwrap().dist;
        let HashPolicy::Any(digests) = strategy.get(dist) else {
            panic!("expected the locked registry wheel to be validated");
        };
        assert_eq!(digests.len(), 1);
        assert_eq!(digests[0].algorithm, HashAlgorithm::Sha256);
        assert_eq!(digests[0].digest.as_ref(), SHA256_HEX);
    }

    #[test]
    fn distribution_without_locked_hash_is_not_verified() {
        let url = "direct+https://example.com/bar-1.0.0-py3-none-any.whl"
            .parse()
            .unwrap();
        let records = [record("bar", UrlOrPath::Url(url), None)];
        let (strategy, required) = strategy_for(&records);

        let dist = &required.values().next().unwrap().dist;
        assert!(matches!(strategy.get(dist), HashPolicy::None));
    }

    #[test]
    fn directory_with_locked_hash_is_not_verified() {
        // uv cannot hash a source tree and hard-fails when a policy demands it.
        // A digest on such a record (hand-edited or foreign lock file) must not produce one.
        let records = [record(
            "local-pkg",
            UrlOrPath::Path(".".into()),
            Some(sha256_hash()),
        )];
        let (strategy, required) = strategy_for(&records);

        let dist = &required.values().next().unwrap().dist;
        assert!(
            matches!(dist, Dist::Source(SourceDist::Directory(_))),
            "fixture should produce a directory dist"
        );
        assert!(matches!(strategy.get(dist), HashPolicy::None));
    }

    #[test]
    fn git_with_locked_hash_is_not_verified() {
        // Same reasoning as for directories: uv rejects hash policies for git sources.
        let url = "git+https://github.com/example/foo?rev=0000000000000000000000000000000000000000#0000000000000000000000000000000000000000"
            .parse()
            .unwrap();
        let records = [record("foo", UrlOrPath::Url(url), Some(sha256_hash()))];
        let (strategy, required) = strategy_for(&records);

        let dist = &required.values().next().unwrap().dist;
        assert!(
            matches!(dist, Dist::Source(SourceDist::Git(_))),
            "fixture should produce a git dist"
        );
        assert!(matches!(strategy.get(dist), HashPolicy::None));
    }

    #[test]
    fn unrelated_distributions_are_not_constrained() {
        // Build dependencies must never be constrained by digests of unrelated locked packages.
        let url = "https://files.pythonhosted.org/packages/foo-1.0.0-py3-none-any.whl"
            .parse()
            .unwrap();
        let records = [record("foo", UrlOrPath::Url(url), Some(sha256_hash()))];
        let (strategy, _) = strategy_for(&records);

        let name = uv_normalize::PackageName::from_str("hatchling").unwrap();
        let version = uv_pep440::Version::from_str("1.25.0").unwrap();
        assert!(matches!(
            strategy.get_package(&name, &version),
            HashPolicy::None
        ));
    }
}
