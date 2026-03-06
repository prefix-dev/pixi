use std::path::{Path, PathBuf};

use pixi_build_backend::generated_recipe::BackendConfig;
use pyo3::{Py, PyAny, Python, pyclass, pymethods};
use pythonize::pythonize;
use serde::Deserialize;
use serde::Deserializer;

#[pyclass]
#[derive(Clone, Debug)]
pub struct PyBackendConfig {
    pub(crate) model: Py<PyAny>,
    pub(crate) debug_dir: Option<PathBuf>,
}

impl<'de> Deserialize<'de> for PyBackendConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TempData(serde_json::Value);

        let mut data = TempData::deserialize(deserializer)?.0;

        Python::attach(|py| {
            let model = pythonize(py, &data).map_err(serde::de::Error::custom)?;

            // Support both debug_dir and debug-dir
            let debug_dir: Option<PathBuf> = data
                .as_object_mut()
                .and_then(|obj| obj.get("debug-dir").or_else(|| obj.get("debug_dir")))
                .and_then(|v| v.as_str().map(PathBuf::from));

            Ok(PyBackendConfig {
                model: model.unbind(),
                debug_dir,
            })
        })
    }
}

#[pymethods]
impl PyBackendConfig {
    #[new]
    fn new(debug_dir: Option<PathBuf>, model: Py<PyAny>) -> Self {
        PyBackendConfig { debug_dir, model }
    }

    fn debug_dir(&self) -> Option<&Path> {
        BackendConfig::debug_dir(self)
    }
}

impl BackendConfig for PyBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        if target_config.debug_dir.is_some() {
            miette::bail!("`debug_dir` cannot have a target specific value");
        }

        Ok(self.clone())
    }
}
