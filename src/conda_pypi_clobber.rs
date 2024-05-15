use std::path::PathBuf;

use distribution_types::CachedDist;
use rattler_conda_types::PrefixRecord;
use uv_interpreter::PythonEnvironment;

use crate::install_wheel::get_wheel_info;

use ahash::{AHashMap, AHashSet};

#[derive(Default, Debug)]
pub(crate) struct PypiCondaClobberRegistry {
    paths_registry: AHashMap<PathBuf, rattler_conda_types::PackageName>,
}

impl PypiCondaClobberRegistry {
    pub fn with_conda_packages(conda_packages: &[PrefixRecord]) -> Self {
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

    pub fn clobber_on_instalation(
        self,
        wheels: Vec<CachedDist>,
        venv: &PythonEnvironment,
    ) -> miette::Result<Option<AHashSet<String>>> {
        let mut clobber_packages: AHashSet<String> = AHashSet::default();

        for wheel in wheels {
            let Ok(Some(whl_info)) = get_wheel_info(wheel.path(), venv) else {
                continue;
            };

            for entry in whl_info.0 {
                let path_to_clobber = whl_info.1.join(entry.path);

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
