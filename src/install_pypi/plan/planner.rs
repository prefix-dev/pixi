use std::{
    collections::{HashMap, hash_map::Entry},
    path::{Path, PathBuf},
};

use super::{
    installation_source::{self, Operation},
    validation::NeedsReinstallError,
};
use itertools::{Either, Itertools};
use pixi_consts::consts;
use std::collections::HashSet;
use uv_cache::Cache;
use uv_distribution_types::{InstalledDist, Name};

use crate::install_pypi::conversions::ConvertToUvDistError;

use super::{
    NeedReinstall, PyPIInstallationPlan, RequiredDists, cache::DistCache,
    installed_dists::InstalledDists, models::ValidateCurrentInstall, validation::need_reinstall,
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
    #[error(transparent)]
    RetrieveDistFromCache(#[from] uv_distribution::Error),
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

    /// Figure out what we can link from the cache locally
    /// and what we need to download from the registry.
    ///
    /// All the 'a lifetimes are to to make sure that the names provided to the CachedDistProvider
    /// are valid for the lifetime of the CachedDistProvider and what is passed to the method
    pub fn plan<'a, Installed: InstalledDists<'a>, Cached: DistCache<'a> + 'a>(
        &self,
        site_packages: &'a Installed,
        mut dist_cache: Cached,
        required_dists: &'a RequiredDists,
    ) -> Result<PyPIInstallationPlan, InstallPlannerError> {
        // Convert RequiredDists to the reference map for internal processing
        let required_dists_map = required_dists.as_ref_map();

        // Packages to be installed directly from the cache
        let mut cached = vec![];
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
            let pkg_and_dist = required_dists_map.get(dist.name());
            // Get the installer name
            let installer = dist
                .installer()
                // Empty string if no installer or any other error
                .map_or(String::new(), |f| f.unwrap_or_default());

            if let Some((required_pkg, required_dist)) = pkg_and_dist {
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
                                reinstalls
                                    .push((dist.clone(), NeedReinstall::ReinstallationRequested));
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
                // Use pre-created dist for cache resolution
                // Okay so we need to re-install the package
                // let's see if we need the remote or local version
                let installation_sources = installation_source::decide_installation_source(
                    &self.uv_cache,
                    required_dist,
                    &mut dist_cache,
                    Operation::Reinstall,
                )
                .map_err(InstallPlannerError::from)?;

                cached.extend(installation_sources.cached);
                remote.extend(installation_sources.remote);
            }
        }

        // Now we need to check if we have any packages left in the required_map
        for (_name, (_pkg, dist)) in required_dists_map
            .iter()
            // Only check the packages that have not been previously installed
            .filter(|(name, _)| !prev_installed_packages.contains(name))
        {
            // Use pre-created dist for cache resolution
            // Okay so we need to re-install the package
            // let's see if we need the remote or local version
            let installation_sources = installation_source::decide_installation_source(
                &self.uv_cache,
                dist,
                &mut dist_cache,
                Operation::Install,
            )
            .map_err(InstallPlannerError::from)?;

            cached.extend(installation_sources.cached);
            remote.extend(installation_sources.remote);
        }

        #[derive(Debug)]
        enum Extraneous<'a> {
            Ours(&'a InstalledDist),
            Theirs,
        }

        // Walk over all installed packages and check if they are required
        let mut extraneous = HashMap::new();
        for dist in site_packages.iter() {
            let pkg_and_dist = required_dists_map.get(dist.name());
            let pkg = pkg_and_dist.map(|(pkg, _dist)| *pkg);
            let installer = dist
                .installer()
                .map_or(String::new(), |f| f.unwrap_or_default());

            match pkg {
                // Apparently we need this package
                // and we have installed
                Some(_) => {}
                // Ignore packages not managed by pixi
                None if installer != consts::PIXI_UV_INSTALLER => {
                    // Do check for doubles though
                    extraneous
                        .entry(dist.name())
                        .or_insert(Vec::new())
                        .push(Extraneous::Theirs);
                }
                // Uninstall unneeded packages
                None => {
                    match extraneous.entry(dist.name()) {
                        // We have already seen this package
                        Entry::Occupied(mut occupied_entry) => {
                            occupied_entry.get_mut().push(Extraneous::Ours(dist));
                        }
                        // This is a new package
                        Entry::Vacant(vacant_entry) => {
                            vacant_entry.insert(vec![Extraneous::Ours(dist)]);
                        }
                    };
                }
            }
        }
        // So it may happen that both conda and PyPI have installed a package with the same name
        // but different versions, in that case, we want to split into extraneous and duplicates
        let (extraneous, duplicates): (Vec<_>, Vec<_>) =
            extraneous.into_iter().partition_map(|(_, dists)| {
                if dists.len() > 1 {
                    Either::Right(
                        dists
                            .into_iter()
                            .filter_map(|d| {
                                if let Extraneous::Ours(dist) = d {
                                    Some(dist.clone())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>(),
                    )
                } else {
                    Either::Left(
                        dists
                            .into_iter()
                            .filter_map(|d| {
                                if let Extraneous::Ours(dist) = d {
                                    Some(dist.clone())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>(),
                    )
                }
            });

        Ok(PyPIInstallationPlan {
            cached,
            remote,
            reinstalls,
            extraneous: extraneous.into_iter().flatten().collect(),
            duplicates: duplicates.into_iter().flatten().collect(),
        })
    }
}
