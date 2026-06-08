use std::str::FromStr;

use rattler_conda_types::RepoDataRecord;

use crate::{
    CacheMetrics, MappingError, derivation::DerivationOutcome, is_conda_forge_record,
    purl::pypi_purl,
};

/// A resolver for conda-forge records where the conda package name is assumed
/// to be the PyPI name.
///
/// This is a last-resort fallback for when the prefix.dev mappings do not know
/// about a conda-forge package.
pub(crate) struct CondaForgeVerbatim;

impl CondaForgeVerbatim {
    pub(crate) async fn derive_conda_forge_verbatim_purls(
        &self,
        record: &RepoDataRecord,
        _cache_metrics: &CacheMetrics,
    ) -> Result<DerivationOutcome, MappingError> {
        if !is_conda_forge_record(record) {
            return Ok(DerivationOutcome::NotApplicable);
        }

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
