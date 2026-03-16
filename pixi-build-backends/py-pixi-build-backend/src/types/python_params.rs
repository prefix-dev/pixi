use pixi_build_backend::generated_recipe::PythonParams;
use pyo3::prelude::*;

#[pyclass]
#[derive(Clone)]
pub struct PyPythonParams {
    pub(crate) inner: PythonParams,
}

impl AsRef<PythonParams> for PyPythonParams {
    fn as_ref(&self) -> &PythonParams {
        &self.inner
    }
}

#[pymethods]
impl PyPythonParams {
    #[new]
    #[pyo3(signature = (editable = false))]
    pub fn new(editable: bool) -> Self {
        PyPythonParams {
            inner: PythonParams { editable },
        }
    }

    #[getter]
    pub fn editable(&self) -> bool {
        self.inner.editable
    }

    #[setter]
    pub fn set_editable(&mut self, editable: bool) {
        self.inner.editable = editable;
    }

    pub fn __repr__(&self) -> String {
        format!("PyPythonParams(editable={})", self.inner.editable)
    }
}

impl From<PythonParams> for PyPythonParams {
    fn from(params: PythonParams) -> Self {
        PyPythonParams { inner: params }
    }
}

impl From<PyPythonParams> for PythonParams {
    fn from(py_params: PyPythonParams) -> Self {
        py_params.inner
    }
}
