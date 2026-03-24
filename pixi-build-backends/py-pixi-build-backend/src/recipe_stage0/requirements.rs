use pixi_build_backend::package_dependency::{PackageDependency, SourceMatchSpec};
use pyo3::{pyclass, pymethods};
use rattler_build_recipe::stage0::SerializableMatchSpec;
use rattler_conda_types::MatchSpec;
use std::fmt::Display;

use pixi_build_backend::specs_conversion::PackageSpecDependencies;

#[pyclass]
#[derive(Clone, Default)]
pub struct PyPackageSpecDependencies {
    pub(crate) inner: PackageSpecDependencies,
}

#[pymethods]
impl PyPackageSpecDependencies {
    #[new]
    pub fn new() -> Self {
        PyPackageSpecDependencies {
            inner: PackageSpecDependencies::default(),
        }
    }

    #[getter]
    pub fn build(&self) -> std::collections::HashMap<String, PyPackageDependency> {
        self.inner
            .build
            .iter()
            .map(|(name, dep)| {
                (
                    name.as_normalized().to_string(),
                    PyPackageDependency { inner: dep.clone() },
                )
            })
            .collect()
    }

    #[getter]
    pub fn host(&self) -> std::collections::HashMap<String, PyPackageDependency> {
        self.inner
            .host
            .iter()
            .map(|(name, dep)| {
                (
                    name.as_normalized().to_string(),
                    PyPackageDependency { inner: dep.clone() },
                )
            })
            .collect()
    }

    #[getter]
    pub fn run(&self) -> std::collections::HashMap<String, PyPackageDependency> {
        self.inner
            .run
            .iter()
            .map(|(name, dep)| {
                (
                    name.as_normalized().to_string(),
                    PyPackageDependency { inner: dep.clone() },
                )
            })
            .collect()
    }

    #[getter]
    pub fn run_constraints(&self) -> std::collections::HashMap<String, PyPackageDependency> {
        self.inner
            .run_constraints
            .iter()
            .map(|(name, dep)| {
                (
                    name.as_normalized().to_string(),
                    PyPackageDependency { inner: dep.clone() },
                )
            })
            .collect()
    }
}

#[pyclass(str)]
#[derive(Clone)]
pub struct PyPackageDependency {
    pub(crate) inner: PackageDependency,
}

#[pymethods]
impl PyPackageDependency {
    #[new]
    pub fn new(matchspec: String) -> pyo3::PyResult<Self> {
        let spec = matchspec.parse::<PackageDependency>().map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid matchspec: {e}"))
        })?;
        Ok(PyPackageDependency { inner: spec })
    }

    #[staticmethod]
    pub fn source(source_matchspec: PySourceMatchSpec) -> Self {
        PyPackageDependency {
            inner: PackageDependency::Source(source_matchspec.inner),
        }
    }

    pub fn is_binary(&self) -> bool {
        matches!(self.inner, PackageDependency::Binary(_))
    }

    pub fn is_source(&self) -> bool {
        matches!(self.inner, PackageDependency::Source(_))
    }

    pub fn get_binary(&self) -> Option<String> {
        match &self.inner {
            PackageDependency::Binary(spec) => Some(spec.to_string()),
            _ => None,
        }
    }

    pub fn get_source(&self) -> Option<PySourceMatchSpec> {
        match &self.inner {
            PackageDependency::Source(spec) => Some(PySourceMatchSpec {
                inner: spec.clone(),
            }),
            _ => None,
        }
    }

    pub fn package_name(&self) -> String {
        self.inner
            .package_name()
            .map(|n| n.as_normalized().to_string())
            .unwrap_or_default()
    }
}

impl Display for PyPackageDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl From<PackageDependency> for PyPackageDependency {
    fn from(dep: PackageDependency) -> Self {
        PyPackageDependency { inner: dep }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PySourceMatchSpec {
    pub(crate) inner: SourceMatchSpec,
}

#[pymethods]
impl PySourceMatchSpec {
    #[new]
    pub fn new(spec: String, location: String) -> pyo3::PyResult<Self> {
        let matchspec = spec.parse::<MatchSpec>().map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid matchspec: {e}"))
        })?;
        let url = location
            .parse()
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("Invalid URL: {e}")))?;

        Ok(PySourceMatchSpec {
            inner: SourceMatchSpec {
                spec: matchspec,
                location: url,
            },
        })
    }

    #[getter]
    pub fn spec(&self) -> String {
        self.inner.spec.to_string()
    }

    #[getter]
    pub fn location(&self) -> String {
        self.inner.location.to_string()
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PySerializableMatchSpec {
    pub(crate) inner: SerializableMatchSpec,
}

#[pymethods]
impl PySerializableMatchSpec {
    #[new]
    pub fn new(spec: String) -> pyo3::PyResult<Self> {
        let matchspec = spec.parse::<MatchSpec>().map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid matchspec: {e}"))
        })?;
        Ok(PySerializableMatchSpec {
            inner: SerializableMatchSpec::from(matchspec),
        })
    }

    #[getter]
    pub fn spec(&self) -> String {
        self.inner.0.to_string()
    }
}

impl From<PackageSpecDependencies> for PyPackageSpecDependencies {
    fn from(deps: PackageSpecDependencies) -> Self {
        PyPackageSpecDependencies { inner: deps }
    }
}

impl From<PyPackageSpecDependencies> for PackageSpecDependencies {
    fn from(py_deps: PyPackageSpecDependencies) -> Self {
        py_deps.inner
    }
}

impl From<PyPackageDependency> for PackageDependency {
    fn from(py_dep: PyPackageDependency) -> Self {
        py_dep.inner
    }
}

impl From<SourceMatchSpec> for PySourceMatchSpec {
    fn from(spec: SourceMatchSpec) -> Self {
        PySourceMatchSpec { inner: spec }
    }
}

impl From<PySourceMatchSpec> for SourceMatchSpec {
    fn from(py_spec: PySourceMatchSpec) -> Self {
        py_spec.inner
    }
}

impl From<SerializableMatchSpec> for PySerializableMatchSpec {
    fn from(spec: SerializableMatchSpec) -> Self {
        PySerializableMatchSpec { inner: spec }
    }
}

impl From<PySerializableMatchSpec> for SerializableMatchSpec {
    fn from(py_spec: PySerializableMatchSpec) -> Self {
        py_spec.inner
    }
}
