//! Verification of locked PyPI artifacts against the hashes in the lock file.
//!
//! The lock file pins registry artifacts (wheels and sdists) to a digest, but
//! uv only checks digests when it is given a [`HashStrategy`] that demands it.
//! This module derives that strategy from the locked records so that:
//!
//! - wheels already present in the uv cache are only reused when their
//!   recorded digest satisfies the locked one (`RegistryWheelIndex` /
//!   `BuiltWheelIndex`), and
//! - artifacts that are downloaded (or read from disk) are hashed and
//!   rejected on a mismatch (`Preparer` / `DistributionDatabase`).

use std::sync::Arc;

use rustc_hash::FxHashMap;
use uv_distribution_types::{DistributionMetadata, VersionId};
use uv_pypi_types::HashDigest;
use uv_types::HashStrategy;

use crate::conversions::package_hashes_to_digests;
use crate::plan::RequiredDists;

/// The expected digests of the locked PyPI artifacts, keyed the way uv
/// identifies distributions.
///
/// Registry distributions are keyed by name and version, direct URL
/// distributions by their URL — both obtained through
/// [`DistributionMetadata::version_id`] on the exact [`uv_distribution_types::Dist`]
/// values that are later installed, so lookups inside uv are guaranteed to hit.
#[derive(Debug, Clone, Default)]
pub struct LockedDistHashes {
    hashes: FxHashMap<VersionId, Vec<HashDigest>>,
}

impl LockedDistHashes {
    /// Collect the locked digests for every required distribution that has
    /// one. Distributions the lock file cannot pin to a digest (git,
    /// directory, and editable path dependencies) are simply absent.
    pub fn from_required_dists(required_dists: &RequiredDists) -> Self {
        let hashes = required_dists
            .values()
            .filter_map(|data| {
                let digests = package_hashes_to_digests(data.record.hash.as_ref()?);
                Some((data.dist.version_id(), digests))
            })
            .collect();
        Self { hashes }
    }

    /// Turn the locked digests into the [`HashStrategy`] handed to uv.
    ///
    /// This uses [`HashStrategy::Verify`] rather than
    /// [`HashStrategy::Require`]: artifacts with a locked digest must match
    /// it, while artifacts without one are installed unverified, mirroring
    /// exactly what the lock file is able to pin.
    pub fn into_verify_strategy(self) -> HashStrategy {
        HashStrategy::Verify(Arc::new(self.hashes))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::str::FromStr;

    use rattler_digest::{Sha256, parse_digest_from_hex};
    use rattler_lock::{PackageHashes, PypiDistributionData, UrlOrPath};
    use uv_distribution_types::HashPolicy;
    use uv_pypi_types::HashAlgorithm;

    use super::*;
    use crate::{InstallablePypiRecord, ManifestData};

    const SHA256_HEX: &str = "5e809d755e8619cb680b5d742cdd911390a377a1cc2e4a0e2b1c7a7cbfb957ff";

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

    fn strategy_for(records: &[InstallablePypiRecord]) -> (HashStrategy, RequiredDists) {
        let required = RequiredDists::from_packages(records.iter(), Path::new(".")).unwrap();
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
    fn unrelated_distributions_are_not_constrained() {
        // Build dependencies resolved during sdist builds must never be
        // rejected because an unrelated locked package shares nothing with
        // them.
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
