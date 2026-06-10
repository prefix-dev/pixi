//! Satisfiability support for [`PypiPackageIdentifier`].
//!
//! The identifier itself lives in `pixi_install_pypi` (it is shared with the
//! PyPI resolve pipeline); this module adds the workspace-side check that a
//! locked PyPI requirement is satisfied by a conda-installed package.

pub use pixi_install_pypi::package_identifier::{ConversionError, PypiPackageIdentifier};

use pixi_consts::consts;
use pixi_uv_conversions::{to_uv_normalize, to_uv_version};

use crate::lock_file::PlatformUnsat;
use crate::lock_file::PlatformUnsat::{
    DirectUrlDependencyOnCondaInstalledPackage, DirectoryDependencyOnCondaInstalledPackage,
    GitDependencyOnCondaInstalledPackage, PathDependencyOnCondaInstalledPackage,
};

/// Extension trait that checks whether a found pypi requirement satisfies
/// the information in a [`PypiPackageIdentifier`].
pub(crate) trait PypiPackageIdentifierSatisfies {
    fn satisfies(
        &self,
        requirement: &uv_distribution_types::Requirement,
    ) -> Result<bool, Box<PlatformUnsat>>;
}

impl PypiPackageIdentifierSatisfies for PypiPackageIdentifier {
    fn satisfies(
        &self,
        requirement: &uv_distribution_types::Requirement,
    ) -> Result<bool, Box<PlatformUnsat>> {
        // Verify the name of the package
        let uv_normalized = to_uv_normalize(self.name.as_normalized())
            .map_err::<ConversionError, _>(From::from)
            .map_err::<PlatformUnsat, _>(From::from)?;
        if uv_normalized != requirement.name {
            return Ok(false);
        }

        // Check the version of the requirement
        match &requirement.source {
            uv_distribution_types::RequirementSource::Registry { specifier, .. } => {
                let uv_version = to_uv_version(&self.version)
                    .map_err::<ConversionError, _>(From::from)
                    .map_err::<PlatformUnsat, _>(From::from)?;
                Ok(specifier.contains(&uv_version))
            }
            // a pypi -> conda requirement on these versions are not supported
            uv_distribution_types::RequirementSource::Url { .. } => {
                tracing::warn!(
                    "PyPI requirement: {} as an url dependency is currently not supported because it is already selected as a conda package",
                    consts::PYPI_PACKAGE_STYLE.apply_to(requirement.name.as_str())
                );
                Err(Box::new(DirectUrlDependencyOnCondaInstalledPackage(
                    requirement.name.clone(),
                )))
            }
            uv_distribution_types::RequirementSource::Git { .. } => {
                tracing::warn!(
                    "PyPI requirement: {} as a Git dependency is currently not supported because it is already selected as a conda package",
                    consts::PYPI_PACKAGE_STYLE.apply_to(requirement.name.as_str())
                );
                Err(Box::new(GitDependencyOnCondaInstalledPackage(
                    requirement.name.clone(),
                )))
            }
            uv_distribution_types::RequirementSource::Path { .. } => {
                tracing::warn!(
                    "PyPI requirement: {} as a path dependency is currently not supported because it is already selected as a conda package",
                    consts::PYPI_PACKAGE_STYLE.apply_to(requirement.name.as_str())
                );
                Err(Box::new(PathDependencyOnCondaInstalledPackage(
                    requirement.name.clone(),
                )))
            }
            uv_distribution_types::RequirementSource::Directory { .. } => {
                tracing::warn!(
                    "PyPI requirement: {} as directory dependency is currently not supported because it is already selected as a conda package",
                    consts::PYPI_PACKAGE_STYLE.apply_to(requirement.name.as_str())
                );
                Err(Box::new(DirectoryDependencyOnCondaInstalledPackage(
                    requirement.name.clone(),
                )))
            }
        }
    }
}
