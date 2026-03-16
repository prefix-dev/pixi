use std::fmt::Display;

use pyo3::prelude::*;
use rattler_conda_types::Platform;

#[pyclass(str)]
#[derive(Clone)]
pub struct PyPlatform {
    pub(crate) inner: Platform,
}

#[pymethods]
impl PyPlatform {
    #[new]
    pub fn new(platform_str: &str) -> PyResult<Self> {
        let platform = platform_str.parse::<Platform>().map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid platform: {e}"))
        })?;
        Ok(PyPlatform { inner: platform })
    }

    #[staticmethod]
    pub fn current() -> Self {
        Platform::current().into()
    }

    #[getter]
    pub fn name(&self) -> String {
        self.inner.to_string()
    }

    #[getter]
    pub fn is_windows(&self) -> bool {
        self.inner.is_windows()
    }

    #[getter]
    pub fn is_linux(&self) -> bool {
        self.inner.is_linux()
    }

    #[getter]
    pub fn is_osx(&self) -> bool {
        self.inner.is_osx()
    }

    #[getter]
    pub fn is_unix(&self) -> bool {
        self.inner.is_unix()
    }

    #[getter]
    pub fn only_platform(&self) -> Option<&str> {
        self.inner.only_platform()
    }
}

impl Display for PyPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl From<Platform> for PyPlatform {
    fn from(platform: Platform) -> Self {
        PyPlatform { inner: platform }
    }
}

impl From<PyPlatform> for Platform {
    fn from(py_platform: PyPlatform) -> Self {
        py_platform.inner
    }
}
