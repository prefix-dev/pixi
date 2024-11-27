use pixi_uv_conversions::{
    to_normalize, to_uv_normalize, to_uv_version, ConversionError as PixiConversionError,
};
use rattler_conda_types::{PackageRecord, PackageUrl, RepoDataRecord};
use std::{collections::HashSet, str::FromStr};
use thiserror::Error;

use pixi_manifest::pypi::PyPiPackageName;
use uv_normalize::{ExtraName, InvalidNameError};

/// Defines information about a Pypi package extracted from either a python
/// package or from a conda package. That can be used for comparison in both
#[derive(Debug)]
pub struct PypiPackageIdentifier {
    pub name: PyPiPackageName,
    pub version: pep440_rs::Version,
    pub extras: HashSet<ExtraName>,
}

impl PypiPackageIdentifier {
    /// Extracts the python packages that will be installed when the specified
    /// conda package is installed.
    pub(crate) fn from_repodata_record(
        record: &RepoDataRecord,
    ) -> Result<Vec<Self>, ConversionError> {
        let mut result = Vec::new();
        Self::from_record_into(record, &mut result)?;

        Ok(result)
    }

    pub fn from_package_record(record: &PackageRecord) -> Result<Vec<Self>, ConversionError> {
        let mut result = Vec::new();
        if let Some(purls) = &record.purls {
            for purl in purls.iter() {
                if let Some(entry) = Self::convert_from_purl(purl, &record.version.as_str())? {
                    result.push(entry);
                }
            }
        }
        Ok(result)
    }

    /// Helper function to write the result of extract the python packages that
    /// will be installed into a pre-allocated vector.
    fn from_record_into(
        record: &RepoDataRecord,
        result: &mut Vec<Self>,
    ) -> Result<(), ConversionError> {
        let mut has_pypi_purl = false;
        let identifiers = Self::from_package_record(&record.package_record)?;
        if !identifiers.is_empty() {
            has_pypi_purl = true;
            result.extend(identifiers);
        }

        // Backwards compatibility:
        // If lock file don't have a purl
        // but the package is a conda-forge package, we just assume that
        // the name of the package is equivalent to the name of the python package.
        // In newer versions of the lock file, we should always have a purl
        // where empty purls means that the package is not a pypi-one.
        if record.package_record.purls.is_none()
            && !has_pypi_purl
            && pypi_mapping::is_conda_forge_record(record)
        {
            tracing::trace!(
                "Using backwards compatibility purl logic for conda package: {}",
                record.package_record.name.as_source()
            );
            // Convert the conda package names to pypi package names. If the conversion fails we
            // just assume that its not a valid python package.
            let name =
                uv_normalize::PackageName::from_str(record.package_record.name.as_source()).ok();
            let version =
                pep440_rs::Version::from_str(&record.package_record.version.as_str()).ok();
            if let (Some(name), Some(version)) = (name, version) {
                let pep_name = to_normalize(&name)?;

                result.push(PypiPackageIdentifier {
                    name: PyPiPackageName::from_normalized(pep_name),
                    version,
                    // TODO: We can't really tell which python extras are enabled in a conda
                    // package.
                    extras: Default::default(),
                })
            }
        }

        Ok(())
    }

    /// Tries to construct an instance from a generic PURL.
    ///
    /// The `fallback_version` is used if the PURL does not contain a version.
    pub(crate) fn convert_from_purl(
        package_url: &PackageUrl,
        fallback_version: &str,
    ) -> Result<Option<Self>, ConversionError> {
        if package_url.package_type() == "pypi" {
            Self::from_pypi_purl(package_url, fallback_version).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Constructs a new instance from a PyPI package URL.
    ///
    /// The `fallback_version` is used if the PURL does not contain a version.
    pub(crate) fn from_pypi_purl(
        package_url: &PackageUrl,
        fallback_version: &str,
    ) -> Result<Self, ConversionError> {
        assert_eq!(package_url.package_type(), "pypi");
        let name = package_url.name();
        let name = uv_normalize::PackageName::from_str(name)
            .map_err(|e| ConversionError::PackageName(name.to_string(), e))?;

        let version_str = package_url.version().unwrap_or(fallback_version);
        let version = pep440_rs::Version::from_str(version_str)
            .map_err(|_| ConversionError::Version(version_str.to_string()))?;

        // TODO: We can't really tell which python extras are enabled from a PURL.
        let extras = HashSet::new();
        let pep_name = to_normalize(&name)?;

        Ok(Self {
            name: PyPiPackageName::from_normalized(pep_name),
            version,
            extras,
        })
    }

    /// Checks of a found pypi requirement satisfies with the information
    /// in this package identifier.
    pub(crate) fn satisfies(
        &self,
        requirement: &uv_pypi_types::Requirement,
    ) -> Result<bool, ConversionError> {
        // Verify the name of the package
        let uv_normalized = to_uv_normalize(self.name.as_normalized())?;
        if uv_normalized != requirement.name {
            return Ok(false);
        }

        // Check the version of the requirement
        match &requirement.source {
            uv_pypi_types::RequirementSource::Registry { specifier, .. } => {
                let uv_version = to_uv_version(&self.version)?;
                Ok(specifier.contains(&uv_version))
            }
            // a pypi -> conda requirement on these versions are not supported
            uv_pypi_types::RequirementSource::Url { .. } => {
                unreachable!("direct url requirement on conda package is not supported")
            }
            uv_pypi_types::RequirementSource::Git { .. } => {
                unreachable!("git requirement on conda package is not supported")
            }
            uv_pypi_types::RequirementSource::Path { .. } => {
                unreachable!("path requirement on conda package is not supported")
            }
            uv_pypi_types::RequirementSource::Directory { .. } => {
                unreachable!("directory requirement on conda package is not supported")
            }
        }
    }
}

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("'{0}' is not a valid python package name")]
    PackageName(String, #[source] InvalidNameError),

    #[error("'{0}' is not a valid python version")]
    Version(String),
    // #[error("'{0}' is not a valid python extra")]
    // Extra(String),
    #[error("Failed to convert to pypi package name")]
    NameConversion(#[from] PixiConversionError),
}
