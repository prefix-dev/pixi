use std::path::Path;

use miette::IntoDiagnostic;
use rattler::install::PythonInfo;

use pixi_consts::consts;
use pixi_utils::prefix::Prefix;
use rattler_lock::{PypiPackageData, PypiPackageEnvironmentData};
use rayon::prelude::*;
use uv_distribution_types::{InstalledDist, Name};

use super::PythonStatus;

/// If the python interpreter is outdated, we need to uninstall all outdated
/// site packages. from the old interpreter.
async fn uninstall_outdated_site_packages(site_packages: &Path) -> miette::Result<()> {
    // Check if the old interpreter is outdated
    let mut dist_dirs = Vec::new();
    for entry in fs_err::read_dir(site_packages).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        if entry.file_type().into_diagnostic()?.is_dir() {
            let path = entry.path();
            dist_dirs.push(path);
        }
    }

    let installed = dist_dirs
        .into_par_iter()
        .flat_map(|path| {
            // Early out if the install was not done by pixi-uv.
            // For performance reasons we only check if the length of the
            // installer file is different from a file containing "uv-pixi".
            let installer_path = path.join("INSTALLER");
            match fs_err::metadata(&installer_path) {
                Ok(metadata) => {
                    if metadata.len() != consts::PIXI_UV_INSTALLER.len() as u64 {
                        // The file is smaller or larger so its contents cannot be "uv-pixi"
                        return None;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // No installer file, so cannot be installed by uv pixi
                    return None;
                }
                _ => {
                    // Unsure, so we continue on the more expensive path
                }
            }

            // Load the information of the installed distribution
            let installed_dist = InstalledDist::try_from_path(&path);
            let Ok(installed_dist) = installed_dist else {
                return None;
            };

            if let Some(installed_dist) = installed_dist {
                // If we can't get the installer, we can't be certain that we have installed it
                let installer = match installed_dist.installer() {
                    Ok(installer) => installer,
                    Err(e) => {
                        tracing::warn!(
                            "could not get installer for {}: {}, will not remove distribution",
                            installed_dist.name(),
                            e
                        );
                        return None;
                    }
                };

                // Only remove if have actually installed it
                // by checking the installer
                if installer.unwrap_or_default() == consts::PIXI_UV_INSTALLER {
                    return Some(installed_dist);
                }
            }

            None
        })
        .collect::<Vec<_>>();

    // Uninstall all packages in old site-packages directory
    for dist_info in installed {
        let _summary = uv_installer::uninstall(&dist_info)
            .await
            .expect("uninstallation of old site-packages failed");
    }

    Ok(())
}

/// Continue or skip the PyPI prefix update.
pub enum ContinuePyPIPrefixUpdate<'a> {
    /// Continue with the PyPI prefix update.
    Continue(&'a PythonInfo),
    /// Skip the PyPI prefix update. Because the python interpreter is removed.
    Skip,
}

/// React on changes to the Python interpreter.
/// Namely we should decide if we want to remove the old site-packages directory.
pub async fn on_python_interpreter_change<'a>(
    status: &'a PythonStatus,
    prefix: &Prefix,
    pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
) -> miette::Result<ContinuePyPIPrefixUpdate<'a>> {
    // If we have changed interpreter, we need to uninstall all site-packages from
    // the old interpreter We need to do this before the pypi prefix update,
    // because that requires a python interpreter.
    match status {
        // If the python interpreter is removed, we need to uninstall all `pixi-uv` site-packages.
        // And we don't need to continue with the rest of the pypi prefix update.
        PythonStatus::Removed { old } => {
            let site_packages_path = prefix.root().join(&old.site_packages_path);
            if site_packages_path.exists() {
                uninstall_outdated_site_packages(&site_packages_path).await?;
            }
            Ok(ContinuePyPIPrefixUpdate::Skip)
        }
        // If the python interpreter is changed, we need to uninstall all site-packages from the old
        // interpreter. And we continue the function to update the pypi packages.
        PythonStatus::Changed { old, new } => {
            // In windows the site-packages path stays the same, so we don't need to
            // uninstall the site-packages ourselves.
            if old.site_packages_path != new.site_packages_path {
                let site_packages_path = prefix.root().join(&old.site_packages_path);
                if site_packages_path.exists() {
                    uninstall_outdated_site_packages(&site_packages_path).await?;
                }
            }
            Ok(ContinuePyPIPrefixUpdate::Continue(new))
        }
        // If the python interpreter is unchanged, and there are no pypi packages to install, we
        // need to remove the site-packages. And we don't need to continue with the rest of
        // the pypi prefix update.
        PythonStatus::Unchanged(info) | PythonStatus::Added { new: info } => {
            if pypi_records.is_empty() {
                let site_packages_path = prefix.root().join(&info.site_packages_path);
                if site_packages_path.exists() {
                    uninstall_outdated_site_packages(&site_packages_path).await?;
                }
                return Ok(ContinuePyPIPrefixUpdate::Skip);
            }
            Ok(ContinuePyPIPrefixUpdate::Continue(info))
        }
        // We can skip the pypi prefix update if there is not python interpreter in the environment.
        PythonStatus::DoesNotExist => Ok(ContinuePyPIPrefixUpdate::Skip),
    }
}
