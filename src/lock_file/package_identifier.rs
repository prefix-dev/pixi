use crate::project::manifest::python::PyPiPackageName;
use crate::pypi_name_mapping;
use pep508_rs::{Requirement, VersionOrUrl};
use rattler_conda_types::{PackageUrl, RepoDataRecord};
use std::{collections::HashSet, str::FromStr};
use thiserror::Error;
use url::Url;
use uv_cache::ArchiveTarget;
use uv_normalize::{ExtraName, InvalidNameError, PackageName};
/// Defines information about a Pypi package extracted from either a python package or from a
/// conda package.
#[derive(Debug)]
pub struct PypiPackageIdentifier {
    pub name: PyPiPackageName,
    pub version: pep440_rs::Version,
    pub url: Url,
    pub extras: HashSet<ExtraName>,
}

impl PypiPackageIdentifier {
    /// Extracts the python packages that will be installed when the specified conda package is
    /// installed.
    pub fn from_record(record: &RepoDataRecord) -> Result<Vec<Self>, ConversionError> {
        let mut result = Vec::new();
        Self::from_record_into(record, &mut result)?;
        Ok(result)
    }

    /// Helper function to write the result of extract the python packages that will be installed
    /// into a pre-allocated vector.
    fn from_record_into(
        record: &RepoDataRecord,
        result: &mut Vec<Self>,
    ) -> Result<(), ConversionError> {
        // Check the PURLs for a python package.
        let mut has_pypi_purl = false;
        for purl in record.package_record.purls.iter() {
            if let Some(entry) = Self::try_from_purl(purl, &record.package_record.version.as_str())?
            {
                result.push(entry);
                has_pypi_purl = true;
            }
        }

        // If there is no pypi purl, but the package is a conda-forge package, we just assume that
        // the name of the package is equivalent to the name of the python package.
        if !has_pypi_purl && pypi_name_mapping::is_conda_forge_record(record) {
            // Convert the conda package names to pypi package names. If the conversion fails we
            // just assume that its not a valid python package.
            let name = PackageName::from_str(record.package_record.name.as_source()).ok();
            let version =
                pep440_rs::Version::from_str(&record.package_record.version.as_str()).ok();
            if let (Some(name), Some(version)) = (name, version) {
                result.push(PypiPackageIdentifier {
                    name: PyPiPackageName::from_normalized(name),
                    version,
                    url: record.url.clone(),
                    // TODO: We can't really tell which python extras are enabled in a conda package.
                    extras: Default::default(),
                })
            }
        }

        Ok(())
    }

    // /// Given a list of conda package records, extract the python packages that will be installed
    // /// when these conda packages are installed.
    // pub fn from_records(records: &[RepoDataRecord]) -> Result<Vec<Self>, ConversionError> {
    //     let mut result = Vec::new();
    //     for record in records {
    //         Self::from_record_into(record, &mut result)?;
    //     }
    //     Ok(result)
    // }

    /// Tries to construct an instance from a generic PURL.
    ///
    /// The `fallback_version` is used if the PURL does not contain a version.
    pub fn try_from_purl(
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
    pub fn from_pypi_purl(
        package_url: &PackageUrl,
        fallback_version: &str,
    ) -> Result<Self, ConversionError> {
        assert_eq!(package_url.package_type(), "pypi");
        let name = package_url.name();
        let name = PackageName::from_str(name)
            .map_err(|e| ConversionError::PackageName(name.to_string(), e))?;
        let version_str = package_url.version().unwrap_or(fallback_version);
        let version = pep440_rs::Version::from_str(version_str)
            .map_err(|_| ConversionError::Version(version_str.to_string()))?;

        // TODO: We can't really tell which python extras are enabled from a PURL.
        let extras = HashSet::new();

        Ok(Self {
            name: PyPiPackageName::from_normalized(name),
            url: Url::parse(&package_url.to_string()).expect("cannot parse purl -> url"),
            version,
            extras,
        })
    }

    pub fn satisfies(&self, requirement: &Requirement) -> bool {
        dbg!(&requirement);
        // Verify the name of the package
        if self.name.as_normalized() != &requirement.name {
            return false;
        }

        // Check the version of the requirement
        match &requirement.version_or_url {
            None => true,
            Some(VersionOrUrl::Url(url)) => {
                // Check if the URL matches
                tracing::warn!("url.to_url:{} == {}:self.url", &url.to_url(), &self.url);
                if &url.to_url() == &self.url {
                    // If the requirement came from a local path, check freshness.
                    // TODO: this uses internals directly from uv, change this once we support direct_url for conda as well, currently it's only supported for pypi
                    if let Ok(archive) = url.to_file_path() {
                        // Convert into UV type
                        let installed = distribution_types::InstalledDist::try_from_path(&archive)
                            .ok()
                            .flatten();
                        if let Some(installed) = installed {
                            // This checks the archive timestamp against the installed timestamp
                            // by comparing a `setup.py`, `pyproject.toml` etc. to the cached version
                            uv_cache::ArchiveTimestamp::up_to_date_with(
                                &archive,
                                uv_cache::ArchiveTarget::Install(&installed),
                            )
                            // In case of an error assume it is not satisfied
                            .unwrap_or(false)
                        } else {
                            false
                        }
                    } else {
                        // Otherwise the we assume that the requirement is satisfied.
                        // because of the url comparison above.
                        true
                    }
                } else {
                    // The URL's do not match
                    false
                }
            }
            Some(VersionOrUrl::VersionSpecifier(required_spec)) => {
                // Check if the locked version is contained in the required version specifier
                required_spec.contains(&self.version)
            }
        }

        // TODO: uv doesn't properly support this yet.
        // // Check if the required extras exist
        // for extra in requirement.extras.iter() {
        //     if !self.extras.contains(extra) {
        //         return false;
        //     }
        // }
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
}
