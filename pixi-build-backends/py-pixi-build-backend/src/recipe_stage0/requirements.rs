use pyo3::{pyclass, pymethods};
use rattler_conda_types::MatchSpec;
use recipe_stage0::matchspec::{PackageDependency, SerializableMatchSpec, SourceMatchSpec};
use recipe_stage0::requirements::{PackageSpecDependencies, Selector};
use std::collections::HashMap;
use std::fmt::Display;

#[pyclass]
#[derive(Clone, Default)]
pub struct PyPackageSpecDependencies {
    pub(crate) inner: PackageSpecDependencies<PackageDependency>,
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
    pub fn build(&self) -> HashMap<String, PyPackageDependency> {
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
    pub fn host(&self) -> HashMap<String, PyPackageDependency> {
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
    pub fn run(&self) -> HashMap<String, PyPackageDependency> {
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
    pub fn run_constraints(&self) -> HashMap<String, PyPackageDependency> {
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
        self.inner.package_name().as_normalized().to_string()
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

#[pyclass]
#[derive(Clone)]
pub struct PySelector {
    pub(crate) inner: Selector,
}

#[pymethods]
impl PySelector {
    #[staticmethod]
    pub fn unix() -> Self {
        PySelector {
            inner: Selector::Unix,
        }
    }

    #[staticmethod]
    pub fn linux() -> Self {
        PySelector {
            inner: Selector::Linux,
        }
    }

    #[staticmethod]
    pub fn win() -> Self {
        PySelector {
            inner: Selector::Win,
        }
    }

    #[staticmethod]
    pub fn macos() -> Self {
        PySelector {
            inner: Selector::MacOs,
        }
    }

    #[staticmethod]
    pub fn platform(platform: String) -> Self {
        PySelector {
            inner: Selector::Platform(platform),
        }
    }

    pub fn is_unix(&self) -> bool {
        matches!(self.inner, Selector::Unix)
    }

    pub fn is_linux(&self) -> bool {
        matches!(self.inner, Selector::Linux)
    }

    pub fn is_win(&self) -> bool {
        matches!(self.inner, Selector::Win)
    }

    pub fn is_macos(&self) -> bool {
        matches!(self.inner, Selector::MacOs)
    }

    pub fn is_platform(&self) -> bool {
        matches!(self.inner, Selector::Platform(_))
    }

    pub fn get_platform(&self) -> Option<String> {
        match &self.inner {
            Selector::Platform(p) => Some(p.clone()),
            _ => None,
        }
    }
}

impl From<PackageSpecDependencies<PackageDependency>> for PyPackageSpecDependencies {
    fn from(deps: PackageSpecDependencies<PackageDependency>) -> Self {
        PyPackageSpecDependencies { inner: deps }
    }
}

impl From<PyPackageSpecDependencies> for PackageSpecDependencies<PackageDependency> {
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

impl From<Selector> for PySelector {
    fn from(selector: Selector) -> Self {
        PySelector { inner: selector }
    }
}

impl From<PySelector> for Selector {
    fn from(py_selector: PySelector) -> Self {
        py_selector.inner
    }
}
