use std::path::Path;

use miette::IntoDiagnostic;

use fancy_display::FancyDisplay;
use pixi_consts::consts;
use pixi_manifest::{EnvironmentName, SystemRequirements};
use pixi_progress::await_in_progress;
use pixi_record::PixiRecord;
use rattler_conda_types::Platform;
use rattler_lock::{PypiIndexes, PypiPackageData, PypiPackageEnvironmentData};
use std::collections::HashMap;
use uv_distribution_types::{InstalledDist, Name};

use crate::{install_pypi, lock_file::UvResolutionContext, prefix::Prefix};

use super::PythonStatus;

/// If the python interpreter is outdated, we need to uninstall all outdated
/// site packages. from the old interpreter.
/// TODO: optimize this by recording the installation of the site-packages to
/// check if this is needed.
async fn uninstall_outdated_site_packages(site_packages: &Path) -> miette::Result<()> {
    // Check if the old interpreter is outdated
    let mut installed = vec![];
    for entry in fs_err::read_dir(site_packages).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        if entry.file_type().into_diagnostic()?.is_dir() {
            let path = entry.path();

            let installed_dist = InstalledDist::try_from_path(&path);
            let Ok(installed_dist) = installed_dist else {
                continue;
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
                        continue;
                    }
                };

                // Only remove if have actually installed it
                // by checking the installer
                if installer.unwrap_or_default() == consts::PIXI_UV_INSTALLER {
                    installed.push(installed_dist);
                }
            }
        }
    }

    // Uninstall all packages in old site-packages directory
    for dist_info in installed {
        let _summary = uv_installer::uninstall(&dist_info)
            .await
            .expect("uninstallation of old site-packages failed");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
// TODO: refactor args into struct
pub async fn update_prefix_pypi(
    environment_name: &EnvironmentName,
    prefix: &Prefix,
    _platform: Platform,
    pixi_records: &[PixiRecord],
    pypi_records: &[(PypiPackageData, PypiPackageEnvironmentData)],
    status: &PythonStatus,
    system_requirements: &SystemRequirements,
    uv_context: &UvResolutionContext,
    pypi_indexes: Option<&PypiIndexes>,
    environment_variables: &HashMap<String, String>,
    lock_file_dir: &Path,
    platform: Platform,
    non_isolated_packages: Option<Vec<String>>,
    no_build: &pixi_manifest::pypi::pypi_options::NoBuild,
) -> miette::Result<()> {
    // If we have changed interpreter, we need to uninstall all site-packages from
    // the old interpreter We need to do this before the pypi prefix update,
    // because that requires a python interpreter.
    let python_info = match status {
        // If the python interpreter is removed, we need to uninstall all `pixi-uv` site-packages.
        // And we don't need to continue with the rest of the pypi prefix update.
        PythonStatus::Removed { old } => {
            let site_packages_path = prefix.root().join(&old.site_packages_path);
            if site_packages_path.exists() {
                uninstall_outdated_site_packages(&site_packages_path).await?;
            }
            return Ok(());
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
            new
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
                return Ok(());
            }
            info
        }
        // We can skip the pypi prefix update if there is not python interpreter in the environment.
        PythonStatus::DoesNotExist => {
            return Ok(());
        }
    };

    // Install and/or remove python packages
    await_in_progress(
        format!(
            "updating pypi packages in '{}'",
            environment_name.fancy_display()
        ),
        |_| {
            install_pypi::update_python_distributions(
                lock_file_dir,
                prefix,
                pixi_records,
                pypi_records,
                &python_info.path,
                system_requirements,
                uv_context,
                pypi_indexes,
                environment_variables,
                platform,
                non_isolated_packages,
                no_build,
            )
        },
    )
    .await
}
