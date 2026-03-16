use std::str::FromStr;

use fs_err as fs;

use pixi_build_types::ProjectModel;
use pyo3::{exceptions::PyValueError, prelude::*};
use pythonize::depythonize;
use rattler_conda_types::Version;
use serde_json::from_str;

#[pyclass]
#[derive(Clone)]
pub struct PyProjectModel {
    pub(crate) inner: ProjectModel,
}

#[pymethods]
impl PyProjectModel {
    #[new]
    #[pyo3(signature = (name, version=None))]
    pub fn new(name: Option<String>, version: Option<String>) -> Self {
        PyProjectModel {
            inner: ProjectModel {
                name,
                version: version.map(|v| {
                    v.parse()
                        .unwrap_or_else(|_| Version::from_str(&v).expect("Invalid version"))
                }),
                targets: None,
                description: None,
                authors: None,
                license: None,
                license_file: None,
                readme: None,
                homepage: None,
                repository: None,
                documentation: None,
                build_number: None,
                build_string: None,
            },
        }
    }

    #[staticmethod]
    pub fn from_json(json: &str) -> PyResult<Self> {
        let project: ProjectModel = from_str(json).map_err(|err| {
            PyErr::new::<PyValueError, _>(format!("Failed to parse ProjectModel from JSON: {err}"))
        })?;

        Ok(PyProjectModel { inner: project })
    }

    #[staticmethod]
    pub fn from_dict(value: &Bound<PyAny>) -> PyResult<Self> {
        let project: ProjectModel = depythonize(value)?;
        Ok(PyProjectModel { inner: project })
    }

    #[staticmethod]
    pub fn from_json_file(path: &str) -> PyResult<Self> {
        let content = fs::read_to_string(path).map_err(|err| {
            PyErr::new::<PyValueError, _>(format!(
                "Failed to read ProjectModel JSON file '{path}': {err}"
            ))
        })?;

        Self::from_json(&content)
    }

    #[getter]
    pub fn name(&self) -> Option<&String> {
        self.inner.name.as_ref()
    }

    #[getter]
    pub fn version(&self) -> Option<String> {
        self.inner.version.as_ref().map(|v| v.to_string())
    }

    #[getter]
    pub fn description(&self) -> Option<String> {
        self.inner.description.clone()
    }

    #[getter]
    pub fn authors(&self) -> Option<Vec<String>> {
        self.inner.authors.clone()
    }

    #[getter]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    #[getter]
    pub fn license_file(&self) -> Option<String> {
        self.inner
            .license_file
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
    }

    #[getter]
    pub fn readme(&self) -> Option<String> {
        self.inner
            .readme
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
    }

    #[getter]
    pub fn homepage(&self) -> Option<String> {
        self.inner.homepage.as_ref().map(|u| u.to_string())
    }

    #[getter]
    pub fn repository(&self) -> Option<String> {
        self.inner.repository.as_ref().map(|u| u.to_string())
    }

    #[getter]
    pub fn documentation(&self) -> Option<String> {
        self.inner.documentation.as_ref().map(|u| u.to_string())
    }

    pub fn _debug_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}

impl From<ProjectModel> for PyProjectModel {
    fn from(model: ProjectModel) -> Self {
        PyProjectModel { inner: model }
    }
}

impl From<&ProjectModel> for PyProjectModel {
    fn from(model: &ProjectModel) -> Self {
        PyProjectModel {
            inner: model.clone(),
        }
    }
}

impl From<PyProjectModel> for ProjectModel {
    fn from(py_model: PyProjectModel) -> Self {
        py_model.inner
    }
}
