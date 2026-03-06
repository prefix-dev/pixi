use crate::{
    create_py_wrap,
    error::PyPixiBuildBackendError,
    recipe_stage0::{
        conditional::{PyItemSource, PyItemString},
        conditional_requirements::PyVecItemPackageDependency,
        requirements::PyPackageSpecDependencies,
    },
    types::{PyPlatform, PyVecString},
};
use ::serde::{Deserialize, Serialize};
use indexmap::IndexMap;
use pyo3::{
    Py, PyResult, Python,
    exceptions::PyValueError,
    pyclass, pymethods,
    types::{PyList, PyListMethods},
};
use rattler_conda_types::package::EntryPoint;
use recipe_stage0::recipe::{
    About, Build, ConditionalRequirements, Extra, IntermediateRecipe, Item, NoArchKind, Package,
    PathSource, Python as RecipePython, Script, Source, Test, UrlSource, Value,
};

use std::fmt::{Display, Formatter};
use std::{collections::HashMap, ops::Deref};

create_py_wrap!(PyHashMapValueString, HashMap<String, PyValueString>, |map: &HashMap<String, PyValueString>, f: &mut Formatter<'_>| {
    write!(f, "{{")?;
    for (k, v) in map {
        write!(f, "{k}: {v}, ")?;
    }
    write!(f, "}}")
});

create_py_wrap!(PyVecItemSource, Vec<PyItemSource>, |vec: &Vec<
    PyItemSource,
>,
                                                     f: &mut Formatter<
    '_,
>| {
    write!(f, "[")?;
    for item in vec {
        write!(f, "{item}, ")?;
    }
    write!(f, "]")
});

create_py_wrap!(
    PyVecTest,
    Vec<PyTest>,
    |vec: &Vec<PyTest>, f: &mut Formatter<'_>| {
        write!(f, "[")?;
        for item in vec {
            write!(f, "{item}, ")?;
        }
        write!(f, "]")
    }
);

create_py_wrap!(
    PyOptionAbout,
    Option<PyAbout>,
    |opt: &Option<PyAbout>, f: &mut Formatter<'_>| {
        match opt {
            Some(about) => write!(f, "{about}"),
            None => write!(f, "None"),
        }
    }
);
create_py_wrap!(
    PyOptionExtra,
    Option<PyExtra>,
    |opt: &Option<PyExtra>, f: &mut Formatter<'_>| {
        match opt {
            Some(extra) => write!(f, "{extra}"),
            None => write!(f, "None"),
        }
    }
);

#[pyclass(get_all, set_all, str)]
#[derive(Clone, Serialize)]
pub struct PyIntermediateRecipe {
    pub context: Py<PyHashMapValueString>,
    pub package: Py<PyPackage>,
    pub source: Py<PyVecItemSource>,
    pub build: Py<PyBuild>,
    pub requirements: Py<PyConditionalRequirements>,
    pub tests: Py<PyVecTest>,
    pub about: Py<PyOptionAbout>,
    pub extra: Py<PyOptionExtra>,
}

impl Display for PyIntermediateRecipe {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ context: {}, package: {}, source: {}, build: {}, requirements: {}, tests: {}, about: {}, extra: {} }}",
            self.context,
            self.package,
            self.source,
            self.build,
            self.requirements,
            self.tests,
            self.about,
            self.extra
        )
    }
}

#[pymethods]
impl PyIntermediateRecipe {
    #[new]
    pub fn new(py: Python) -> PyResult<Self> {
        Ok(PyIntermediateRecipe {
            context: Py::new(py, PyHashMapValueString::default())?,
            package: Py::new(py, PyPackage::default())?,
            source: Py::new(py, PyVecItemSource::default())?,
            build: Py::new(py, PyBuild::new(py))?,
            requirements: Py::new(py, PyConditionalRequirements::new(py))?,
            tests: Py::new(py, PyVecTest::default())?,
            about: Py::new(py, PyOptionAbout::default())?,
            extra: Py::new(py, PyOptionExtra::default())?,
        })
    }
    /// Creates a recipe from YAML string
    #[staticmethod]
    pub fn from_yaml(yaml: String, py: Python) -> PyResult<Self> {
        // Preprocess the YAML to handle script content as both String and Vec<String>
        let preprocessed_yaml = Self::preprocess_script_content(yaml)?;

        let intermediate_recipe: IntermediateRecipe = serde_yaml::from_str(&preprocessed_yaml)
            .map_err(PyPixiBuildBackendError::YamlSerialization)?;

        let py_intermediate_recipe =
            PyIntermediateRecipe::from_intermediate_recipe(intermediate_recipe, py);

        Ok(py_intermediate_recipe)
    }

    /// Converts the PyIntermediateRecipe to a YAML string.
    pub fn to_yaml(&self, py: Python) -> PyResult<String> {
        let recipe = self.to_intermediate_recipe(py);
        Ok(serde_yaml::to_string(&recipe).map_err(PyPixiBuildBackendError::YamlSerialization)?)
    }
}

impl PyIntermediateRecipe {
    /// Preprocess YAML to convert script content from Vec<String> to String
    fn preprocess_script_content(yaml: String) -> PyResult<String> {
        use serde_yaml::Value;

        let mut value: Value =
            serde_yaml::from_str(&yaml).map_err(PyPixiBuildBackendError::YamlSerialization)?;

        // Navigate to build.script.content
        if let Value::Mapping(ref mut map) = value
            && let Some(Value::Mapping(build_map)) = map.get_mut("build")
            && let Some(Value::Mapping(script_map)) = build_map.get_mut("script")
            && let Some(content_value) = script_map.get_mut("content")
        {
            // If content is a sequence (Vec), convert to String
            if let &mut Value::Sequence(ref content_seq) = content_value {
                let content_strings: Vec<String> = content_seq
                    .iter()
                    .filter_map(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect();

                *content_value = Value::String(content_strings.join("\n"));
            }
            // If it's already a String, leave it as is
        }

        Ok(serde_yaml::to_string(&value).map_err(PyPixiBuildBackendError::YamlSerialization)?)
    }

    pub fn from_intermediate_recipe(recipe: IntermediateRecipe, py: Python) -> Self {
        // Convert context (IndexMap<String, Value<String>>) to PyHashMap
        let context_map = recipe
            .context
            .into_iter()
            .map(|(k, v)| (k, v.into()))
            .collect::<HashMap<String, PyValueString>>();

        let py_context = PyHashMapValueString { inner: context_map };

        // Convert package
        let py_package: PyPackage = recipe.package.into();

        // Convert source (ConditionalList<Source>) to PyVecItemSource
        let py_sources: Vec<PyItemSource> =
            recipe.source.into_iter().map(|item| item.into()).collect();

        let py_vec_source: PyVecItemSource = py_sources.into();

        // Convert build
        let py_build: PyBuild = PyBuild::from_build(py, recipe.build);

        // Convert requirements
        let py_requirements: PyConditionalRequirements =
            PyConditionalRequirements::from_conditional_requirements(py, recipe.requirements);

        // Convert tests
        let py_tests: Vec<PyTest> = recipe.tests.into_iter().map(|test| test.into()).collect();
        let py_vec_tests: PyVecTest = py_tests.into();

        // Convert about (Option<About>)
        let py_about = PyOptionAbout {
            inner: recipe.about.map(|about| about.into()),
        };

        // Convert extra (Option<Extra>)
        let py_extra = PyOptionExtra {
            inner: recipe.extra.map(|extra| extra.into()),
        };

        PyIntermediateRecipe {
            context: Py::new(py, py_context).unwrap(),
            package: Py::new(py, py_package).unwrap(),
            source: Py::new(py, py_vec_source).unwrap(),
            build: Py::new(py, py_build).unwrap(),
            requirements: Py::new(py, py_requirements).unwrap(),
            tests: Py::new(py, py_vec_tests).unwrap(),
            about: Py::new(py, py_about).unwrap(),
            extra: Py::new(py, py_extra).unwrap(),
        }
    }

    pub fn to_intermediate_recipe(&self, py: Python) -> IntermediateRecipe {
        let context: HashMap<String, PyValueString> = (*self.context.borrow(py).clone()).clone();
        let context = context
            .into_iter()
            .map(|(k, v)| (k, (*v).clone()))
            .collect();

        let py_package = self.package.borrow(py).clone();
        let package: Package = (*py_package).clone();

        let source: Vec<Item<Source>> = (*self.source.borrow(py).clone())
            .clone()
            .into_iter()
            .map(|item| (*item).clone())
            .collect();

        let build: Build = self.build.borrow(py).clone().into_build(py);
        let requirements: ConditionalRequirements = self
            .requirements
            .borrow(py)
            .clone()
            .into_conditional_requirements(py);

        let tests: Vec<Test> = (*self.tests.borrow(py).clone())
            .clone()
            .into_iter()
            .map(|test| (*test).clone())
            .collect();

        let about: Option<About> = (*self.about.borrow(py).clone())
            .clone()
            .map(|about| (*about).clone());
        let extra: Option<Extra> = (*self.extra.borrow(py).clone())
            .clone()
            .map(|extra| (*extra).clone());

        IntermediateRecipe {
            context,
            package,
            source,
            build,
            requirements,
            tests,
            about,
            extra,
        }
    }
}

#[pyclass(str)]
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PyPackage {
    pub(crate) inner: Package,
}

impl Deref for PyPackage {
    type Target = Package;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Display for PyPackage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[pymethods]
impl PyPackage {
    #[new]
    pub fn new(name: PyValueString, version: PyValueString) -> Self {
        PyPackage {
            inner: Package {
                name: name.inner,
                version: version.inner,
            },
        }
    }

    #[getter]
    pub fn name(&self) -> PyValueString {
        PyValueString {
            inner: self.inner.name.clone(),
        }
    }

    #[setter]
    pub fn set_name(&mut self, name: PyValueString) {
        self.inner.name = name.inner;
    }

    #[getter]
    pub fn version(&self) -> PyValueString {
        PyValueString {
            inner: self.inner.version.clone(),
        }
    }

    #[setter]
    pub fn set_version(&mut self, version: PyValueString) {
        self.inner.version = version.inner;
    }
}

impl From<Package> for PyPackage {
    fn from(package: Package) -> Self {
        PyPackage { inner: package }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PySource {
    pub(crate) inner: Source,
}

#[pymethods]
impl PySource {
    #[staticmethod]
    pub fn url(url_source: PyUrlSource) -> Self {
        PySource {
            inner: Source::Url(url_source.inner),
        }
    }

    #[staticmethod]
    pub fn path(path_source: PyPathSource) -> Self {
        PySource {
            inner: Source::Path(path_source.inner),
        }
    }

    pub fn is_url(&self) -> bool {
        matches!(self.inner, Source::Url(_))
    }

    pub fn is_path(&self) -> bool {
        matches!(self.inner, Source::Path(_))
    }
}

impl From<Source> for PySource {
    fn from(source: Source) -> Self {
        PySource { inner: source }
    }
}

impl From<PySource> for Source {
    fn from(py_source: PySource) -> Self {
        py_source.inner
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyUrlSource {
    pub(crate) inner: UrlSource,
}

#[pymethods]
impl PyUrlSource {
    #[new]
    pub fn new(url: String, sha256: Option<String>) -> PyResult<Self> {
        Ok(PyUrlSource {
            inner: UrlSource {
                url: url
                    .parse()
                    .map_err(|e| PyValueError::new_err(format!("Invalid URL: {e}")))?,
                sha256: sha256.map(Value::Concrete),
            },
        })
    }

    #[getter]
    pub fn url(&self) -> String {
        self.inner.url.to_string()
    }

    #[getter]
    pub fn sha256(&self) -> Option<String> {
        self.inner
            .sha256
            .clone()
            .and_then(|v| v.concrete().cloned())
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyPathSource {
    pub(crate) inner: PathSource,
}

#[pymethods]
impl PyPathSource {
    #[new]
    pub fn new(path: String, sha256: Option<String>) -> Self {
        PyPathSource {
            inner: PathSource {
                path: Value::Concrete(path),
                sha256: sha256.map(Value::Concrete),
            },
        }
    }

    #[getter]
    pub fn path(&self) -> String {
        self.inner.path.to_string()
    }

    #[getter]
    pub fn sha256(&self) -> Option<String> {
        self.inner
            .sha256
            .clone()
            .and_then(|v| v.concrete().cloned())
    }
}

create_py_wrap!(PyOptionValueU64, Option<PyValueU64>, |opt: &Option<
    PyValueU64,
>,
                                                       f: &mut Formatter<
    '_,
>| {
    match opt {
        Some(value) => write!(f, "{value}"),
        None => write!(f, "None"),
    }
});

create_py_wrap!(
    PyOptionPyNoArchKind,
    Option<PyNoArchKind>,
    |opt: &Option<PyNoArchKind>, f: &mut Formatter<'_>| {
        match opt {
            Some(value) => write!(f, "{value}"),
            None => write!(f, "None"),
        }
    }
);

#[pyclass(get_all, set_all, str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyBuild {
    pub number: Py<PyOptionValueU64>,
    pub script: Py<PyScript>,
    pub noarch: Py<PyOptionPyNoArchKind>,
    pub python: Py<PyPython>,
}

impl PyBuild {
    pub fn into_build(self, py: Python) -> Build {
        let noarch = self
            .noarch
            .borrow(py)
            .clone()
            .as_ref()
            .map(|n| n.clone().inner);

        Build {
            number: self
                .number
                .borrow(py)
                .clone()
                .as_ref()
                .map(|n| n.deref().clone()),
            script: self.script.borrow(py).clone().into_script(py),
            noarch,
            python: self.python.borrow(py).inner.clone(),
        }
    }

    pub fn from_build(py: Python, build: Build) -> PyBuild {
        let py_value = build.number.map(PyValueU64::from);
        let py_value: PyOptionValueU64 = py_value.into();

        let py_noarch = build.noarch.map(PyNoArchKind::from);

        let py_noarch_value: PyOptionPyNoArchKind = py_noarch.into();

        PyBuild {
            number: Py::new(py, py_value).unwrap(),
            script: Py::new(py, PyScript::from_script(py, build.script)).unwrap(),
            noarch: Py::new(py, py_noarch_value).unwrap(),
            python: Py::new(py, Into::<PyPython>::into(build.python)).unwrap(),
        }
    }
}

#[pymethods]
impl PyBuild {
    #[new]
    pub fn new(py: Python) -> Self {
        PyBuild {
            number: Py::new(py, PyOptionValueU64::default()).unwrap(),
            script: Py::new(py, PyScript::new(py, None, None, None)).unwrap(),
            noarch: Py::new(py, PyOptionPyNoArchKind::default()).unwrap(),
            python: Py::new(py, PyPython::new(None).unwrap()).unwrap(),
        }
    }
}

impl Display for PyBuild {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ number: {}, script: {}, noarch: {}, python: {} }}",
            self.number, self.script, self.noarch, self.python
        )
    }
}

create_py_wrap!(PyHashMap, HashMap<String, String>, |map: &HashMap<String, String>, f: &mut Formatter<'_>| {
    write!(f, "{{")?;
    for (k, v) in map {
        write!(f, "{k}: {v}, ")?;
    }
    write!(f, "}}")
});

#[pyclass(get_all, set_all, str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyScript {
    pub content: String,
    pub env: Py<PyHashMap>,
    pub secrets: Py<PyVecString>,
}

impl Display for PyScript {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ content: {}, env: {}, secrets: {} }}",
            self.content, self.env, self.secrets
        )
    }
}

#[pymethods]
impl PyScript {
    #[new]
    pub fn new(
        py: Python,
        content: Option<String>,
        env: Option<HashMap<String, String>>,
        secrets: Option<Vec<String>>,
    ) -> Self {
        let content = content.unwrap_or_default();
        let env = env.map(PyHashMap::from).unwrap_or_default();
        let secrets = secrets.map(PyVecString::from).unwrap_or_default();

        PyScript {
            content,
            env: Py::new(py, env).unwrap(),
            secrets: Py::new(py, secrets).unwrap(),
        }
    }
}

impl PyScript {
    pub fn into_script(self, py: Python) -> Script {
        Script {
            content: self.content,
            env: (*self.env.borrow(py).clone())
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
            secrets: self.secrets.borrow(py).inner.clone(),
        }
    }

    pub fn from_script(py: Python, script: Script) -> Self {
        let py_hashmap: PyHashMap =
            PyHashMap::from(script.env.into_iter().collect::<HashMap<_, _>>());

        let secrets_vec: PyVecString = script.secrets.into();

        PyScript {
            content: script.content,
            env: Py::new(py, py_hashmap).unwrap(),
            secrets: Py::new(py, secrets_vec).unwrap(),
        }
    }
}

#[pyclass(str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyPython {
    pub(crate) inner: RecipePython,
}

#[pymethods]
impl PyPython {
    #[new]
    pub fn new(entry_points: Option<Vec<String>>) -> PyResult<Self> {
        let entry_points: Result<Vec<EntryPoint>, _> = entry_points
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.parse())
            .collect();

        match entry_points {
            Ok(entry_points) => Ok(PyPython {
                inner: RecipePython { entry_points },
            }),
            Err(_) => Err(pyo3::exceptions::PyValueError::new_err(
                "Invalid entry point format",
            )),
        }
    }

    #[getter]
    pub fn entry_points(&self) -> Vec<String> {
        self.inner
            .entry_points
            .iter()
            .map(|e| e.to_string())
            .collect()
    }

    #[setter]
    pub fn set_entry_points(&mut self, entry_points: Vec<String>) -> PyResult<()> {
        let entry_points: Result<Vec<EntryPoint>, _> =
            entry_points.into_iter().map(|s| s.parse()).collect();

        match entry_points {
            Ok(entry_points) => {
                self.inner.entry_points = entry_points;
                Ok(())
            }
            Err(_) => Err(pyo3::exceptions::PyValueError::new_err(
                "Invalid entry point format",
            )),
        }
    }
}

impl From<RecipePython> for PyPython {
    fn from(python: RecipePython) -> Self {
        PyPython { inner: python }
    }
}

impl Display for PyPython {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[pyclass(str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyNoArchKind {
    pub(crate) inner: NoArchKind,
}

impl Display for PyNoArchKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[pymethods]
impl PyNoArchKind {
    #[staticmethod]
    pub fn python() -> Self {
        PyNoArchKind {
            inner: NoArchKind::Python,
        }
    }

    #[staticmethod]
    pub fn generic() -> Self {
        PyNoArchKind {
            inner: NoArchKind::Generic,
        }
    }

    pub fn is_python(&self) -> bool {
        matches!(self.inner, NoArchKind::Python)
    }

    pub fn is_generic(&self) -> bool {
        matches!(self.inner, NoArchKind::Generic)
    }
}

impl From<NoArchKind> for PyNoArchKind {
    fn from(noarch: NoArchKind) -> Self {
        PyNoArchKind { inner: noarch }
    }
}

macro_rules! create_py_value {
    ($name: ident, $type: ident) => {
        #[pyclass(str)]
        #[derive(Clone, Serialize, Deserialize)]
        pub struct $name {
            pub(crate) inner: Value<$type>,
        }

        #[pymethods]
        impl $name {
            #[new]
            pub fn new(value: String) -> Self {
                $name {
                    inner: value.parse().unwrap(),
                }
            }

            #[staticmethod]
            pub fn concrete(value: $type) -> Self {
                $name {
                    inner: Value::Concrete(value),
                }
            }

            #[staticmethod]
            pub fn template(template: String) -> Self {
                $name {
                    inner: Value::Template(template),
                }
            }

            pub fn is_concrete(&self) -> bool {
                matches!(self.inner, Value::Concrete(_))
            }

            pub fn is_template(&self) -> bool {
                matches!(self.inner, Value::Template(_))
            }

            pub fn get_concrete(&self) -> Option<$type> {
                match &self.inner {
                    Value::Concrete(v) => Some(v.clone()),
                    _ => None,
                }
            }

            pub fn get_template(&self) -> Option<String> {
                match &self.inner {
                    Value::Template(t) => Some(t.clone()),
                    _ => None,
                }
            }
        }

        impl From<Value<$type>> for $name {
            fn from(value: Value<$type>) -> Self {
                $name { inner: value }
            }
        }

        impl Deref for $name {
            type Target = Value<$type>;
            fn deref(&self) -> &Self::Target {
                &self.inner
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.inner)
            }
        }
    };
}

create_py_value!(PyValueString, String);
create_py_value!(PyValueU64, u64);

#[pyclass(str, get_all, set_all)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyConditionalRequirements {
    // pub(crate) inner: ConditionalRequirements,
    // #[serde(default)]
    pub(crate) build: Py<PyVecItemPackageDependency>,
    // #[serde(default)]
    pub(crate) host: Py<PyVecItemPackageDependency>,
    // #[serde(default)]
    pub(crate) run: Py<PyVecItemPackageDependency>,
    // #[serde(default)]
    pub(crate) run_constraints: Py<PyVecItemPackageDependency>,
}

#[pymethods]
impl PyConditionalRequirements {
    #[new]
    pub fn new(py: Python) -> Self {
        let build = PyVecItemPackageDependency::new();
        let host = PyVecItemPackageDependency::new();
        let run = PyVecItemPackageDependency::new();
        let run_constraints = PyVecItemPackageDependency::new();

        PyConditionalRequirements {
            build: Py::new(py, build).unwrap(),
            host: Py::new(py, host).unwrap(),
            run: Py::new(py, run).unwrap(),
            run_constraints: Py::new(py, run_constraints).unwrap(),
        }
    }

    pub fn resolve(
        &self,
        py: Python,
        host_platform: Option<&PyPlatform>,
    ) -> PyPackageSpecDependencies {
        let build = self.build.borrow(py).clone();
        // let build = *build;

        // let build: Vec<Item<PackageDependency>> = *(self.build.borrow(py).clone());
        let host = self.host.borrow(py).clone();
        let run = self.run.borrow(py).clone();
        let run_constraints = self.run_constraints.borrow(py).clone();

        let platform = host_platform.map(|p| p.inner);

        let resolved = ConditionalRequirements::resolve(
            &build.inner,
            &host.inner,
            &run.inner,
            &run_constraints.inner,
            platform,
        );

        resolved.into()
    }
}

impl PyConditionalRequirements {
    pub fn into_conditional_requirements(self, py: Python) -> ConditionalRequirements {
        ConditionalRequirements {
            build: (self.build.borrow(py).clone())
                .inner
                .clone()
                .into_iter()
                .collect(),
            host: (self.host.borrow(py).clone())
                .inner
                .clone()
                .into_iter()
                .collect(),
            run: (self.run.borrow(py).clone())
                .inner
                .clone()
                .into_iter()
                .collect(),
            run_constraints: (self.run_constraints.borrow(py).clone())
                .inner
                .clone()
                .into_iter()
                .collect(),
        }
    }

    pub fn from_conditional_requirements(
        py: Python,
        requirements: ConditionalRequirements,
    ) -> Self {
        let build: PyVecItemPackageDependency = requirements.build.into();
        let host: PyVecItemPackageDependency = requirements.host.into();
        let run: PyVecItemPackageDependency = requirements.run.into();
        let run_constraints: PyVecItemPackageDependency = requirements.run_constraints.into();

        PyConditionalRequirements {
            build: Py::new(py, build).unwrap(),
            host: Py::new(py, host).unwrap(),
            run: Py::new(py, run).unwrap(),
            run_constraints: Py::new(py, run_constraints).unwrap(),
        }
    }
}

impl Display for PyConditionalRequirements {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{ build: {} }}", self.build)?;
        write!(f, "{{ host: {} }}", self.host)?;
        write!(f, "{{ run: {} }}", self.run)?;
        write!(f, "{{ run_constraints: {} }}", self.run_constraints)?;
        Ok(())
    }
}

create_py_wrap!(PyTest, Test);

#[pyclass(str)]
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PyAbout {
    pub(crate) inner: About,
}

#[pymethods]
impl PyAbout {
    #[new]
    pub fn new() -> Self {
        PyAbout {
            inner: About::default(),
        }
    }

    #[getter]
    pub fn homepage(&self) -> Option<String> {
        self.inner
            .homepage
            .clone()
            .and_then(|v| v.concrete().cloned())
    }

    #[getter]
    pub fn license(&self) -> Option<String> {
        self.inner
            .license
            .clone()
            .and_then(|v| v.concrete().cloned())
    }

    #[getter]
    pub fn summary(&self) -> Option<String> {
        self.inner
            .summary
            .clone()
            .and_then(|v| v.concrete().cloned())
    }

    #[getter]
    pub fn description(&self) -> Option<String> {
        self.inner
            .description
            .clone()
            .and_then(|v| v.concrete().cloned())
    }
}

impl Display for PyAbout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{ {} }}", self.inner)
    }
}

impl From<About> for PyAbout {
    fn from(about: About) -> Self {
        PyAbout { inner: about }
    }
}

impl Deref for PyAbout {
    type Target = About;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[pyclass(str)]
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PyExtra {
    pub(crate) inner: Extra,
}

#[pymethods]
impl PyExtra {
    #[new]
    pub fn new() -> Self {
        PyExtra {
            inner: Extra::default(),
        }
    }

    #[getter]
    pub fn recipe_maintainers(&self) -> PyResult<Py<PyList>> {
        Python::attach(|py| {
            let list = PyList::empty(py);
            for dep in &self.inner.recipe_maintainers {
                list.append(PyItemString { inner: dep.clone() })?;
            }
            Ok(list.unbind())
        })
    }
}

impl From<Extra> for PyExtra {
    fn from(extra: Extra) -> Self {
        PyExtra { inner: extra }
    }
}

impl Deref for PyExtra {
    type Target = Extra;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Display for PyExtra {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{ {} }}", self.inner)
    }
}
