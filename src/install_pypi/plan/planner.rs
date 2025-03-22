use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::{reasons, validation::NeedsReinstallError};
use pixi_consts::consts;
use pixi_uv_conversions::to_uv_version;
use rattler_lock::PypiPackageData;
use std::collections::HashSet;
use uv_cache::Cache;
use uv_distribution_types::{CachedDist, Dist, Name};

use crate::install_pypi::conversions::{convert_to_dist, ConvertToUvDistError};

use super::{
    models::ValidateCurrentInstall,
    providers::{CachedDistProvider, InstalledDistProvider},
    reasons::OperationToReason,
    validation::need_reinstall,
    InstallReason, NeedReinstall, PyPIInstallPlan,
};

/// Struct that handles the planning of the installation
/// of the PyPI packages into an existing conda environment with specific
/// locked data
///
/// When executing the [`InstallPlanner::plan`] method, we will figure out what
/// we can link from the cache locally and what we need to download from the registry.
/// As well as determine what we need to remove, which we call extraneous packages.
///
/// This is all inspired by the structs and methods in the uv crate, specifically the `uv_installer` module.
/// But all of it is heavily modified as we need to use our locked data for comparison, and also ignore some things
/// that uv would usually act on.
pub struct InstallPlanner {
    uv_cache: Cache,
    lock_file_dir: PathBuf,
}

#[derive(thiserror::Error, Debug)]
pub enum InstallPlannerError {
    #[error(transparent)]
    DetermineReinstall(#[from] NeedsReinstallError),
    #[error(transparent)]
    ConvertToUvDist(#[from] ConvertToUvDistError),
    #[error(transparent)]
    UvConversion(#[from] pixi_uv_conversions::ConversionError),
}

impl InstallPlanner {
    pub fn new(uv_cache: Cache, lock_file_dir: impl AsRef<Path>) -> Self {
        Self {
            uv_cache,
            lock_file_dir: lock_file_dir.as_ref().to_path_buf(),
        }
    }

    #[cfg(test)]
    /// Set the refresh policy for the UV cache
    pub fn with_uv_refresh(self, refresh: uv_cache::Refresh) -> Self {
        Self {
            uv_cache: self.uv_cache.with_refresh(refresh),
            lock_file_dir: self.lock_file_dir.clone(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Decide if we need to get the distribution from the local cache or the registry
    /// this method will add the distribution to the local or remote vector,
    /// depending on whether the version is stale, available locally or not
    fn decide_installation_source<'a, Op: OperationToReason>(
        &self,
        name: &'a uv_normalize::PackageName,
        required_pkg: &PypiPackageData,
        local: &mut Vec<(CachedDist, InstallReason)>,
        remote: &mut Vec<(Dist, InstallReason)>,
        dist_cache: &mut impl CachedDistProvider<'a>,
        op_to_reason: Op,
    ) -> Result<(), InstallPlannerError> {
        // Okay so we need to re-install the package
        // let's see if we need the remote or local version

        // First, check if we need to revalidate the package
        // then we should get it from the remote
        if self.uv_cache.must_revalidate_package(name) {
            remote.push((
                convert_to_dist(required_pkg, &self.lock_file_dir)?,
                op_to_reason.stale(),
            ));
            return Ok(());
        }
        let uv_version = to_uv_version(&required_pkg.version)?;
        // If it is not stale its either in the registry cache or not
        let cached = dist_cache.get_cached_dist(name, uv_version);
        // If we have it in the cache we can use that
        if let Some(distribution) = cached {
            local.push((CachedDist::Registry(distribution), op_to_reason.cached()));
        // If we don't have it in the cache we need to download it
        } else {
            remote.push((
                convert_to_dist(required_pkg, &self.lock_file_dir)?,
                op_to_reason.missing(),
            ));
        }

        Ok(())
    }

    /// Figure out what we can link from the cache locally
    /// and what we need to download from the registry.
    /// Also determine what we need to remove.
    ///
    /// All the 'a lifetimes are to to make sure that the names provided to the CachedDistProvider
    /// are valid for the lifetime of the CachedDistProvider and what is passed to the method
    pub fn plan<'a, Installed: InstalledDistProvider<'a>, Cached: CachedDistProvider<'a> + 'a>(
        &self,
        site_packages: &'a Installed,
        mut dist_cache: Cached,
        required_pkgs: &'a HashMap<uv_normalize::PackageName, &PypiPackageData>,
    ) -> Result<PyPIInstallPlan, InstallPlannerError> {
        // Packages to be removed
        let mut extraneous = vec![];
        // Packages to be installed directly from the cache
        let mut local = vec![];
        // Try to install from the registry or direct url or w/e
        let mut remote = vec![];
        // Packages that need to be reinstalled
        // i.e. need to be removed before being installed
        let mut reinstalls = vec![];

        // Will contain the packages that have been previously installed
        // and a decision has been made what to do with them
        let mut prev_installed_packages = HashSet::new();

        // Walk over all installed packages and check if they are required
        for dist in site_packages.iter() {
            // Check if we require the package to be installed
            let pkg = required_pkgs.get(dist.name());
            // Get the installer name
            let installer = dist
                .installer()
                // Empty string if no installer or any other error
                .map_or(String::new(), |f| f.unwrap_or_default());

            match pkg {
                Some(required_pkg) => {
                    // Add to the list of previously installed packages
                    prev_installed_packages.insert(dist.name());
                    // Check if we need this package installed but it is not currently installed by us
                    if installer != consts::PIXI_UV_INSTALLER {
                        // We are managing the package but something else has installed a version
                        // let's re-install to make sure that we have the **correct** version
                        reinstalls.push((
                            dist.clone(),
                            NeedReinstall::InstallerMismatch {
                                previous_installer: installer.clone(),
                            },
                        ));
                    } else {
                        // Check if we need to reinstall
                        match need_reinstall(dist, required_pkg, &self.lock_file_dir)? {
                            ValidateCurrentInstall::Keep => {
                                //
                                if self.uv_cache.must_revalidate_package(dist.name()) {
                                    reinstalls.push((
                                        dist.clone(),
                                        NeedReinstall::ReinstallationRequested,
                                    ));
                                } else {
                                    // No need to reinstall
                                    continue;
                                }
                            }
                            ValidateCurrentInstall::Reinstall(reason) => {
                                reinstalls.push((dist.clone(), reason));
                            }
                        }
                    }
                    // Okay so we need to re-install the package
                    // let's see if we need the remote or local version
                    self.decide_installation_source(
                        dist.name(),
                        required_pkg,
                        &mut local,
                        &mut remote,
                        &mut dist_cache,
                        reasons::Reinstall,
                    )?;
                }
                // Second case we are not managing the package
                None if installer != consts::PIXI_UV_INSTALLER => {
                    // Ignore packages that we are not managed by us
                    continue;
                }
                // Third case we *are* managing the package but it is no longer required
                None => {
                    // Add to the extraneous list
                    // as we do manage it but have no need for it
                    extraneous.push(dist.clone());
                }
            }
        }

        // Now we need to check if we have any packages left in the required_map
        for (name, pkg) in required_pkgs
            .iter()
            // Only check the packages that have not been previously installed
            .filter(|(name, _)| !prev_installed_packages.contains(name))
        {
            // Decide if we need to get the distribution from the local cache or the registry
            self.decide_installation_source(
                name,
                pkg,
                &mut local,
                &mut remote,
                &mut dist_cache,
                reasons::Install,
            )?;
        }

        Ok(PyPIInstallPlan {
            local,
            remote,
            reinstalls,
            extraneous,
        })
    }
}
