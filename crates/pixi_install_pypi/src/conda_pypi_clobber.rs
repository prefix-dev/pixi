use std::path::{Path, PathBuf};

use pixi_path::normalize_std;
use rattler_conda_types::PrefixRecord;
use uv_distribution_types::CachedDist;
use uv_python::PythonEnvironment;

use ahash::{AHashMap, AHashSet};

use super::install_wheel::get_wheel_info;

#[derive(Default, Debug)]
pub(crate) struct PypiCondaClobberRegistry {
    /// A registry of the paths of the installed conda paths and the package names
    paths_registry: AHashMap<PathBuf, rattler_conda_types::PackageName>,
}

fn conda_relative_wheel_record_path(
    site_packages_dir: &Path,
    record_path: impl AsRef<Path>,
    prefix_root: &Path,
) -> Option<PathBuf> {
    normalize_std(&site_packages_dir.join(record_path))
        .strip_prefix(prefix_root)
        .ok()
        .map(Path::to_path_buf)
}

impl PypiCondaClobberRegistry {
    /// Register the paths of the installed conda packages
    /// to later check if they are going to be clobbered by the installation of the wheels
    pub(crate) fn with_conda_packages(conda_packages: &[PrefixRecord]) -> Self {
        let mut registry = AHashMap::with_capacity(conda_packages.len() * 50);
        for record in conda_packages {
            for path in &record.paths_data.paths {
                registry.insert(
                    path.relative_path.clone(),
                    record.repodata_record.package_record.name.clone(),
                );
            }
        }
        Self {
            paths_registry: registry,
        }
    }

    /// Check if the installation of the wheels is going to clobber any installed conda package
    /// and return the names of the packages that are going to be clobbered
    /// this allow to warn the user about the overwriting of already installed packages
    /// in case of wrong mapping data
    /// or malicious packages
    pub(crate) fn clobber_on_installation(
        self,
        wheels: Vec<CachedDist>,
        venv: &PythonEnvironment,
        prefix_root: &Path,
    ) -> miette::Result<Option<AHashSet<String>>> {
        let mut clobber_packages: AHashSet<String> = AHashSet::default();

        for wheel in wheels {
            let Ok(Some(whl_info)) = get_wheel_info(wheel.path(), venv) else {
                continue;
            };

            for entry in whl_info.0 {
                let Some(path_to_clobber) =
                    conda_relative_wheel_record_path(&whl_info.1, entry.path, prefix_root)
                else {
                    continue;
                };

                if let Some(name) = self.paths_registry.get(&path_to_clobber) {
                    clobber_packages.insert(name.as_normalized().to_string());
                }
            }
        }
        if clobber_packages.is_empty() {
            return Ok(None);
        }
        Ok(Some(clobber_packages))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::conda_relative_wheel_record_path;

    #[test]
    fn record_path_escaping_site_packages_is_matched_prefix_relative() {
        let prefix = Path::new("/prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");

        assert_eq!(
            conda_relative_wheel_record_path(&site_packages, "../../../bin/prek", prefix),
            Some(PathBuf::from("bin/prek"))
        );
    }

    #[test]
    fn record_path_outside_prefix_is_ignored() {
        let prefix = Path::new("/prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");

        assert_eq!(
            conda_relative_wheel_record_path(&site_packages, "../../../../../bin/prek", prefix),
            None
        );
    }
}
