use std::str::FromStr;

use rattler_conda_types::RepoDataRecord;

use crate::{CacheMetrics, MappingError, derivation::DerivationOutcome, purl::pypi_purl};

/// A resolver that assumes the conda package name is the PyPI name.
///
/// This is a last-resort heuristic for when mapping data does not know about a
/// package. Whether the heuristic is allowed is decided by the derivation mode
/// before this resolver is called.
pub(crate) struct SameName;

impl SameName {
    pub(crate) async fn derive_same_name_purls(
        &self,
        record: &RepoDataRecord,
        _cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        // Try to convert the name and version into pep440/pep508 compliant versions.
        let (Some(name), Some(_version)) = (
            pep508_rs::PackageName::from_str(record.package_record.name.as_source()).ok(),
            pep440_rs::Version::from_str(&record.package_record.version.as_str()).ok(),
        ) else {
            // If we cannot convert the name or version, we cannot build a purl.
            return Ok(DerivationOutcome::NoPurls);
        };

        Ok(DerivationOutcome::Purls(vec![pypi_purl(
            name.to_string(),
            None,
        )]))
    }
}
