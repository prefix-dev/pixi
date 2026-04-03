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
use indexmap::IndexMap;
use pixi_build_backend::package_dependency::PackageDependency;
use pyo3::{
    Py, PyResult, Python,
    exceptions::PyValueError,
    pyclass, pymethods,
    types::{PyList, PyListMethods},
};
use rattler_build_recipe::stage0::source::{PathSource, UrlSource};
use rattler_build_recipe::stage0::{
    About, Build, Extra, Item, License, Package, PythonBuild, Requirements, Script,
    SerializableMatchSpec, SingleOutputRecipe, Source, TestType, Value,
};
use rattler_conda_types::{NoArchType, VersionWithSource, package::EntryPoint};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::path::PathBuf;
use url::Url;

// --------------------------------------------------------------------------
// Helper: convert between ConditionalList<SerializableMatchSpec> and
// Vec<Item<PackageDependency>> so that the Python-facing API stays in terms
// of PackageDependency while the inner recipe uses SerializableMatchSpec.
// --------------------------------------------------------------------------

fn matchspec_items_to_dep_items(
    items: &rattler_build_recipe::stage0::ConditionalList<SerializableMatchSpec>,
) -> Vec<Item<PackageDependency>> {
    items.iter().map(convert_item_spec_to_dep).collect()
}

fn convert_item_spec_to_dep(item: &Item<SerializableMatchSpec>) -> Item<PackageDependency> {
    match item {
        Item::Value(value) => {
            if let Some(concrete) = value.as_concrete() {
                Item::Value(Value::new_concrete(
                    PackageDependency::from(concrete.clone()),
                    None,
                ))
            } else if let Some(template) = value.as_template() {
                Item::Value(Value::new_template(template.clone(), None))
            } else {
                // Should not happen
                Item::Value(Value::new_concrete(
                    PackageDependency::from(SerializableMatchSpec::from("unknown")),
                    None,
                ))
            }
        }
        Item::Conditional(cond) => {
            let then_items: Vec<Item<PackageDependency>> =
                cond.then.iter().map(convert_item_spec_to_dep).collect();
            let else_items = cond.else_value.as_ref().map(|els| {
                let items: Vec<Item<PackageDependency>> =
                    els.iter().map(convert_item_spec_to_dep).collect();
                rattler_build_recipe::stage0::NestedItemList::new(items)
            });
            Item::Conditional(rattler_build_recipe::stage0::Conditional {
                condition: cond.condition.clone(),
                then: rattler_build_recipe::stage0::NestedItemList::new(then_items),
                else_value: else_items,
                condition_span: None,
            })
        }
    }
}

fn dep_items_to_matchspec_items(
    items: &[Item<PackageDependency>],
) -> rattler_build_recipe::stage0::ConditionalList<SerializableMatchSpec> {
    let converted: Vec<Item<SerializableMatchSpec>> =
        items.iter().map(convert_item_dep_to_spec).collect();
    rattler_build_recipe::stage0::ConditionalList::new(converted)
}

fn convert_item_dep_to_spec(item: &Item<PackageDependency>) -> Item<SerializableMatchSpec> {
    match item {
        Item::Value(value) => {
            if let Some(concrete) = value.as_concrete() {
                Item::Value(Value::new_concrete(
                    SerializableMatchSpec::from(concrete.clone()),
                    None,
                ))
            } else if let Some(template) = value.as_template() {
                Item::Value(Value::new_template(template.clone(), None))
            } else {
                Item::Value(Value::new_concrete(
                    SerializableMatchSpec::from("unknown"),
                    None,
                ))
            }
        }
        Item::Conditional(cond) => {
            let then_items: Vec<Item<SerializableMatchSpec>> =
                cond.then.iter().map(convert_item_dep_to_spec).collect();
            let else_items = cond.else_value.as_ref().map(|els| {
                let items: Vec<Item<SerializableMatchSpec>> =
                    els.iter().map(convert_item_dep_to_spec).collect();
                rattler_build_recipe::stage0::NestedItemList::new(items)
            });
            Item::Conditional(rattler_build_recipe::stage0::Conditional {
                condition: cond.condition.clone(),
                then: rattler_build_recipe::stage0::NestedItemList::new(then_items),
                else_value: else_items,
                condition_span: None,
            })
        }
    }
}

// --------------------------------------------------------------------------
// Wrapper types
// --------------------------------------------------------------------------

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

// --------------------------------------------------------------------------
// PyIntermediateRecipe — wraps SingleOutputRecipe
// --------------------------------------------------------------------------

#[pyclass(get_all, set_all, str)]
#[derive(Clone)]
pub struct PyIntermediateRecipe {
    pub context: Py<PyHashMapValueString>,
    pub package: Py<PyPackage>,
    pub source: Py<PyVecItemSource>,
    pub build: Py<PyBuild>,
    pub requirements: Py<PyConditionalRequirements>,
    pub tests: Py<PyVecTest>,
    pub about: Py<PyAbout>,
    pub extra: Py<PyExtra>,
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
            about: Py::new(py, PyAbout::new())?,
            extra: Py::new(py, PyExtra::new())?,
        })
    }
    /// Creates a recipe from YAML string
    #[staticmethod]
    pub fn from_yaml(yaml: String, py: Python) -> PyResult<Self> {
        let recipe = rattler_build_recipe::stage0::parse_recipe_from_source(&yaml)
            .map_err(|e| PyValueError::new_err(format!("failed to parse recipe: {e}")))?;

        let py_recipe = PyIntermediateRecipe::from_single_output_recipe(recipe, py);

        Ok(py_recipe)
    }

    /// Converts the PyIntermediateRecipe to a YAML string.
    pub fn to_yaml(&self, py: Python) -> PyResult<String> {
        let recipe = self.to_single_output_recipe(py);
        Ok(serde_yaml::to_string(&recipe).map_err(PyPixiBuildBackendError::YamlSerialization)?)
    }
}

impl PyIntermediateRecipe {
    pub fn from_single_output_recipe(recipe: SingleOutputRecipe, py: Python) -> Self {
        // Convert context (IndexMap<String, Value<Variable>>) to PyHashMap
        // We treat Variable values as strings for the Python API
        let context_map = recipe
            .context
            .into_iter()
            .map(|(k, v)| {
                let py_val = if let Some(concrete) = v.as_concrete() {
                    PyValueString {
                        inner: Value::new_concrete(concrete.to_string(), None),
                    }
                } else if let Some(template) = v.as_template() {
                    PyValueString {
                        inner: Value::new_template(template.clone(), None),
                    }
                } else {
                    PyValueString {
                        inner: Value::new_concrete(String::new(), None),
                    }
                };
                (k, py_val)
            })
            .collect::<HashMap<String, PyValueString>>();

        let py_context = PyHashMapValueString { inner: context_map };

        // Convert package
        let py_package = PyPackage::from_package(recipe.package);

        // Convert source (ConditionalList<Source>) to PyVecItemSource
        let py_sources: Vec<PyItemSource> =
            recipe.source.into_iter().map(|item| item.into()).collect();
        let py_vec_source: PyVecItemSource = py_sources.into();

        // Convert build
        let py_build = PyBuild::from_build(py, recipe.build);

        // Convert requirements
        let py_requirements = PyConditionalRequirements::from_requirements(py, recipe.requirements);

        // Convert tests
        let py_tests: Vec<PyTest> = recipe.tests.into_iter().map(|test| test.into()).collect();
        let py_vec_tests: PyVecTest = py_tests.into();

        // Convert about (now a direct field, not Option)
        let py_about = PyAbout::from_about(recipe.about);

        // Convert extra (now a direct field, not Option)
        let py_extra = PyExtra::from_extra(recipe.extra);

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

    pub fn to_single_output_recipe(&self, py: Python) -> SingleOutputRecipe {
        let context: HashMap<String, PyValueString> = (*self.context.borrow(py).clone()).clone();
        let context: IndexMap<String, Value<rattler_build_jinja::Variable>> = context
            .into_iter()
            .map(|(k, v)| {
                let val = if let Some(concrete) = v.inner.as_concrete() {
                    Value::new_concrete(rattler_build_jinja::Variable::from(concrete.clone()), None)
                } else if let Some(template) = v.inner.as_template() {
                    Value::new_template(template.clone(), None)
                } else {
                    Value::new_concrete(rattler_build_jinja::Variable::from(String::new()), None)
                };
                (k, val)
            })
            .collect();

        let py_package = self.package.borrow(py).clone();
        let package = py_package.to_package();

        let source: Vec<Item<Source>> = (*self.source.borrow(py).clone())
            .clone()
            .into_iter()
            .map(|item| (*item).clone())
            .collect();
        let source = rattler_build_recipe::stage0::ConditionalList::new(source);

        let build: Build = self.build.borrow(py).clone().into_build(py);
        let requirements: Requirements = self.requirements.borrow(py).clone().into_requirements(py);

        let tests: Vec<Item<TestType>> = (*self.tests.borrow(py).clone())
            .clone()
            .into_iter()
            .map(|test| (*test).clone())
            .collect();
        let tests = rattler_build_recipe::stage0::ConditionalList::new(tests);

        let about: About = self.about.borrow(py).clone().to_about();
        let extra: Extra = self.extra.borrow(py).clone().to_extra();

        SingleOutputRecipe {
            schema_version: Some(1),
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

// --------------------------------------------------------------------------
// PyPackage — wraps Package (name: Value<PackageName>, version: Value<VersionWithSource>)
// Python sees name/version as strings.
// --------------------------------------------------------------------------

#[pyclass(str)]
#[derive(Clone)]
pub struct PyPackage {
    name: Value<rattler_conda_types::SourcePackageName>,
    version: Value<VersionWithSource>,
}

impl Default for PyPackage {
    fn default() -> Self {
        PyPackage {
            name: Value::new_concrete(
                rattler_conda_types::SourcePackageName::from(
                    rattler_conda_types::PackageName::new_unchecked("unnamed"),
                ),
                None,
            ),
            version: Value::new_concrete("0.0.0".parse::<VersionWithSource>().unwrap(), None),
        }
    }
}

impl Display for PyPackage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name_str = self
            .name
            .as_concrete()
            .map(|n| n.to_string())
            .unwrap_or_else(|| {
                self.name
                    .as_template()
                    .map(|t| t.to_string())
                    .unwrap_or_default()
            });
        let version_str = self
            .version
            .as_concrete()
            .map(|v| v.to_string())
            .unwrap_or_else(|| {
                self.version
                    .as_template()
                    .map(|t| t.to_string())
                    .unwrap_or_default()
            });
        write!(f, "{{ name: {name_str}, version: {version_str} }}")
    }
}

#[pymethods]
impl PyPackage {
    #[new]
    pub fn new(name: PyValueString, version: PyValueString) -> PyResult<Self> {
        let pkg_name = pyvalue_string_to_package_name(&name)?;
        let pkg_version = pyvalue_string_to_version(&version)?;
        Ok(PyPackage {
            name: pkg_name,
            version: pkg_version,
        })
    }

    #[getter]
    pub fn name(&self) -> PyValueString {
        package_name_to_pyvalue_string(&self.name)
    }

    #[setter]
    pub fn set_name(&mut self, name: PyValueString) -> PyResult<()> {
        self.name = pyvalue_string_to_package_name(&name)?;
        Ok(())
    }

    #[getter]
    pub fn version(&self) -> PyValueString {
        version_to_pyvalue_string(&self.version)
    }

    #[setter]
    pub fn set_version(&mut self, version: PyValueString) -> PyResult<()> {
        self.version = pyvalue_string_to_version(&version)?;
        Ok(())
    }
}

impl PyPackage {
    fn from_package(package: Package) -> Self {
        PyPackage {
            name: package.name,
            version: package.version,
        }
    }

    fn to_package(&self) -> Package {
        Package {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }
}

// Conversion helpers for Package fields
fn pyvalue_string_to_package_name(
    val: &PyValueString,
) -> PyResult<Value<rattler_conda_types::SourcePackageName>> {
    if let Some(concrete) = val.inner.as_concrete() {
        let pkg_name: rattler_conda_types::PackageName = concrete
            .parse()
            .map_err(|e| PyValueError::new_err(format!("invalid package name: {e}")))?;
        Ok(Value::new_concrete(
            rattler_conda_types::SourcePackageName::from(pkg_name),
            None,
        ))
    } else if let Some(template) = val.inner.as_template() {
        Ok(Value::new_template(template.clone(), None))
    } else {
        Err(PyValueError::new_err("value must be concrete or template"))
    }
}

fn package_name_to_pyvalue_string(
    val: &Value<rattler_conda_types::SourcePackageName>,
) -> PyValueString {
    if let Some(concrete) = val.as_concrete() {
        PyValueString {
            inner: Value::new_concrete(concrete.to_string(), None),
        }
    } else if let Some(template) = val.as_template() {
        PyValueString {
            inner: Value::new_template(template.clone(), None),
        }
    } else {
        PyValueString {
            inner: Value::new_concrete(String::new(), None),
        }
    }
}

fn pyvalue_string_to_version(val: &PyValueString) -> PyResult<Value<VersionWithSource>> {
    if let Some(concrete) = val.inner.as_concrete() {
        let version: VersionWithSource = concrete
            .parse()
            .map_err(|e| PyValueError::new_err(format!("invalid version: {e}")))?;
        Ok(Value::new_concrete(version, None))
    } else if let Some(template) = val.inner.as_template() {
        Ok(Value::new_template(template.clone(), None))
    } else {
        Err(PyValueError::new_err("value must be concrete or template"))
    }
}

fn version_to_pyvalue_string(val: &Value<VersionWithSource>) -> PyValueString {
    if let Some(concrete) = val.as_concrete() {
        PyValueString {
            inner: Value::new_concrete(concrete.to_string(), None),
        }
    } else if let Some(template) = val.as_template() {
        PyValueString {
            inner: Value::new_template(template.clone(), None),
        }
    } else {
        PyValueString {
            inner: Value::new_concrete(String::new(), None),
        }
    }
}

// --------------------------------------------------------------------------
// PySource, PyUrlSource, PyPathSource — wraps rattler-build source types
// --------------------------------------------------------------------------

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
                url: vec![Value::new_concrete(url, None)],
                sha256: sha256.map(|s| {
                    let hash: rattler_digest::Sha256Hash =
                        rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&s)
                            .expect("invalid sha256 hash");
                    Value::new_concrete(hash, None)
                }),
                md5: None,
                file_name: None,
                patches: rattler_build_recipe::stage0::ConditionalList::default(),
                target_directory: None,
                attestation: None,
            },
        })
    }

    #[getter]
    pub fn url(&self) -> String {
        self.inner
            .url
            .first()
            .and_then(|v| v.as_concrete().cloned())
            .unwrap_or_default()
    }

    #[getter]
    pub fn sha256(&self) -> Option<String> {
        self.inner
            .sha256
            .as_ref()
            .and_then(|v| v.as_concrete())
            .map(|h| format!("{h:x}"))
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
                path: Value::new_concrete(PathBuf::from(path), None),
                sha256: sha256.map(|s| {
                    let hash: rattler_digest::Sha256Hash =
                        rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&s)
                            .expect("invalid sha256 hash");
                    Value::new_concrete(hash, None)
                }),
                md5: None,
                patches: rattler_build_recipe::stage0::ConditionalList::default(),
                target_directory: None,
                file_name: None,
                use_gitignore: true,
                filter: rattler_build_recipe::stage0::IncludeExclude::default(),
            },
        }
    }

    #[getter]
    pub fn path(&self) -> String {
        self.inner
            .path
            .as_concrete()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    #[getter]
    pub fn sha256(&self) -> Option<String> {
        self.inner
            .sha256
            .as_ref()
            .and_then(|v| v.as_concrete())
            .map(|h| format!("{h:x}"))
    }
}

// --------------------------------------------------------------------------
// PyBuild — wraps Build (only commonly used fields exposed)
// --------------------------------------------------------------------------

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
        let noarch: Option<Value<NoArchType>> = self
            .noarch
            .borrow(py)
            .clone()
            .as_ref()
            .map(|n| n.to_value_noarch());

        let number: Option<Value<u64>> = self
            .number
            .borrow(py)
            .clone()
            .as_ref()
            .map(|n| n.inner.clone());

        let script: Script = self.script.borrow(py).clone().into_script(py);

        let python: PythonBuild = self.python.borrow(py).clone().into_python_build();

        Build {
            string: None,
            number,
            script,
            noarch,
            python,
            ..Build::default()
        }
    }

    pub fn from_build(py: Python, build: Build) -> PyBuild {
        let py_number = build.number.map(PyValueU64::from);
        let py_number: PyOptionValueU64 = py_number.into();

        let py_noarch = build.noarch.map(PyNoArchKind::from_value_noarch);
        let py_noarch_value: PyOptionPyNoArchKind = py_noarch.into();

        PyBuild {
            number: Py::new(py, py_number).unwrap(),
            script: Py::new(py, PyScript::from_script(py, build.script)).unwrap(),
            noarch: Py::new(py, py_noarch_value).unwrap(),
            python: Py::new(py, PyPython::from_python_build(build.python)).unwrap(),
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

// --------------------------------------------------------------------------
// PyScript — wraps Script
// content: Option<ConditionalList<String>> → Python sees as String
// env: IndexMap<String, Value<String>> → Python sees as HashMap<String, String>
// --------------------------------------------------------------------------

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
        let content = if self.content.is_empty() {
            None
        } else {
            // Wrap the content string into a ConditionalList<String>
            Some(rattler_build_recipe::stage0::ConditionalList::new(vec![
                Item::Value(Value::new_concrete(self.content, None)),
            ]))
        };

        let env: IndexMap<String, Value<String>> = (*self.env.borrow(py).clone())
            .clone()
            .into_iter()
            .map(|(k, v)| (k, Value::new_concrete(v, None)))
            .collect();

        Script {
            content,
            env,
            secrets: self.secrets.borrow(py).inner.clone(),
            ..Script::default()
        }
    }

    pub fn from_script(py: Python, script: Script) -> Self {
        // Extract content: join all concrete string items with newlines
        let content = script
            .content
            .as_ref()
            .map(|cl| {
                cl.iter()
                    .filter_map(|item| {
                        if let Item::Value(val) = item {
                            val.as_concrete().cloned()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<String>>()
                    .join("\n")
            })
            .unwrap_or_default();

        // Extract env: get concrete values from Value<String>
        let env_map: HashMap<String, String> = script
            .env
            .into_iter()
            .filter_map(|(k, v)| v.as_concrete().cloned().map(|c| (k, c)))
            .collect();
        let py_hashmap = PyHashMap::from(env_map);

        let secrets_vec: PyVecString = script.secrets.into();

        PyScript {
            content,
            env: Py::new(py, py_hashmap).unwrap(),
            secrets: Py::new(py, secrets_vec).unwrap(),
        }
    }
}

// --------------------------------------------------------------------------
// PyPython — wraps PythonBuild
// --------------------------------------------------------------------------

#[pyclass(str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyPython {
    entry_points: Vec<EntryPoint>,
    version_independent: bool,
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
                entry_points,
                version_independent: false,
            }),
            Err(_) => Err(pyo3::exceptions::PyValueError::new_err(
                "Invalid entry point format",
            )),
        }
    }

    #[getter]
    pub fn entry_points(&self) -> Vec<String> {
        self.entry_points.iter().map(|e| e.to_string()).collect()
    }

    #[setter]
    pub fn set_entry_points(&mut self, entry_points: Vec<String>) -> PyResult<()> {
        let entry_points: Result<Vec<EntryPoint>, _> =
            entry_points.into_iter().map(|s| s.parse()).collect();

        match entry_points {
            Ok(entry_points) => {
                self.entry_points = entry_points;
                Ok(())
            }
            Err(_) => Err(pyo3::exceptions::PyValueError::new_err(
                "Invalid entry point format",
            )),
        }
    }
}

impl PyPython {
    fn from_python_build(python: PythonBuild) -> Self {
        // Extract concrete entry points from ConditionalList<EntryPoint>
        let entry_points: Vec<EntryPoint> = python
            .entry_points
            .iter()
            .filter_map(|item| {
                if let Item::Value(val) = item {
                    val.as_concrete().cloned()
                } else {
                    None
                }
            })
            .collect();

        let version_independent = python
            .version_independent
            .and_then(|v| v.as_concrete().copied())
            .unwrap_or(false);

        PyPython {
            entry_points,
            version_independent,
        }
    }

    fn into_python_build(self) -> PythonBuild {
        let entry_points = rattler_build_recipe::stage0::ConditionalList::new(
            self.entry_points
                .into_iter()
                .map(|ep| Item::Value(Value::new_concrete(ep, None)))
                .collect(),
        );

        let version_independent = if self.version_independent {
            Some(Value::new_concrete(true, None))
        } else {
            None
        };

        PythonBuild {
            entry_points,
            version_independent,
            ..PythonBuild::default()
        }
    }
}

impl Display for PyPython {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{ entry_points: [")?;
        for (i, ep) in self.entry_points.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{ep}")?;
        }
        write!(f, "] }}")
    }
}

// --------------------------------------------------------------------------
// PyNoArchKind — wraps NoArchType (wrapped in Value)
// --------------------------------------------------------------------------

#[pyclass(str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyNoArchKind {
    noarch_type: NoArchType,
}

impl Display for PyNoArchKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.noarch_type.is_python() {
            write!(f, "python")
        } else if self.noarch_type.is_generic() {
            write!(f, "generic")
        } else {
            write!(f, "unknown")
        }
    }
}

#[pymethods]
impl PyNoArchKind {
    #[staticmethod]
    pub fn python() -> Self {
        PyNoArchKind {
            noarch_type: NoArchType::python(),
        }
    }

    #[staticmethod]
    pub fn generic() -> Self {
        PyNoArchKind {
            noarch_type: NoArchType::generic(),
        }
    }

    pub fn is_python(&self) -> bool {
        self.noarch_type.is_python()
    }

    pub fn is_generic(&self) -> bool {
        self.noarch_type.is_generic()
    }
}

impl PyNoArchKind {
    fn from_value_noarch(val: Value<NoArchType>) -> Self {
        let noarch_type = val.as_concrete().cloned().unwrap_or(NoArchType::generic());
        PyNoArchKind { noarch_type }
    }

    fn to_value_noarch(&self) -> Value<NoArchType> {
        Value::new_concrete(self.noarch_type, None)
    }
}

// --------------------------------------------------------------------------
// PyValueString / PyValueU64 — wraps Value<String> / Value<u64>
// --------------------------------------------------------------------------

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
                // Try to parse as template first (contains ${{)
                if value.contains("${{") {
                    if let Ok(template) =
                        rattler_build_recipe::stage0::JinjaTemplate::new(value.clone())
                    {
                        return $name {
                            inner: Value::new_template(template, None),
                        };
                    }
                }
                // Otherwise create as concrete value
                // Parse the string into the target type T, then wrap in Value
                let parsed: $type = value.parse().unwrap();
                $name {
                    inner: Value::new_concrete(parsed, None),
                }
            }

            #[staticmethod]
            pub fn concrete(value: $type) -> Self {
                $name {
                    inner: Value::new_concrete(value, None),
                }
            }

            #[staticmethod]
            pub fn template(template: String) -> Self {
                let tmpl = rattler_build_recipe::stage0::JinjaTemplate::new(template.clone())
                    .unwrap_or_else(|_| {
                        rattler_build_recipe::stage0::JinjaTemplate::new(format!(
                            "${{{{ {template} }}}}"
                        ))
                        .unwrap()
                    });
                $name {
                    inner: Value::new_template(tmpl, None),
                }
            }

            pub fn is_concrete(&self) -> bool {
                self.inner.is_concrete()
            }

            pub fn is_template(&self) -> bool {
                self.inner.is_template()
            }

            pub fn get_concrete(&self) -> Option<$type> {
                self.inner.as_concrete().cloned()
            }

            pub fn get_template(&self) -> Option<String> {
                self.inner.as_template().map(|t| t.to_string())
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
                if let Some(concrete) = self.inner.as_concrete() {
                    write!(f, "{concrete}")
                } else if let Some(template) = self.inner.as_template() {
                    write!(f, "{template}")
                } else {
                    write!(f, "<unknown>")
                }
            }
        }
    };
}

create_py_value!(PyValueString, String);
create_py_value!(PyValueU64, u64);

// --------------------------------------------------------------------------
// PyConditionalRequirements — wraps Requirements
// Python API keeps Vec<Item<PackageDependency>>, converts to/from
// ConditionalList<SerializableMatchSpec> for the inner recipe type.
// --------------------------------------------------------------------------

#[pyclass(str, get_all, set_all)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyConditionalRequirements {
    pub(crate) build: Py<PyVecItemPackageDependency>,
    pub(crate) host: Py<PyVecItemPackageDependency>,
    pub(crate) run: Py<PyVecItemPackageDependency>,
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
        // For now, just flatten the requirements into PackageSpecDependencies
        // by extracting concrete PackageDependency values
        let _platform = host_platform.map(|p| p.inner);
        let build_deps = self.build.borrow(py).clone();
        let host_deps = self.host.borrow(py).clone();
        let run_deps = self.run.borrow(py).clone();
        let run_constraints_deps = self.run_constraints.borrow(py).clone();

        let resolve_list = |items: &[Item<PackageDependency>]| -> indexmap::IndexMap<
            rattler_conda_types::PackageName,
            PackageDependency,
        > {
            let mut result = indexmap::IndexMap::new();
            for item in items {
                if let Item::Value(val) = item
                    && let Some(dep) = val.as_concrete()
                    && let Some(name) = dep.package_name()
                {
                    result.insert(name.clone(), dep.clone());
                }
            }
            result
        };

        let resolved = pixi_build_backend::specs_conversion::PackageSpecDependencies {
            build: resolve_list(&build_deps.inner),
            host: resolve_list(&host_deps.inner),
            run: resolve_list(&run_deps.inner),
            run_constraints: resolve_list(&run_constraints_deps.inner),
        };

        resolved.into()
    }
}

impl PyConditionalRequirements {
    pub fn into_requirements(self, py: Python) -> Requirements {
        let build_items = self.build.borrow(py).clone();
        let host_items = self.host.borrow(py).clone();
        let run_items = self.run.borrow(py).clone();
        let run_constraints_items = self.run_constraints.borrow(py).clone();

        Requirements {
            build: dep_items_to_matchspec_items(&build_items.inner),
            host: dep_items_to_matchspec_items(&host_items.inner),
            run: dep_items_to_matchspec_items(&run_items.inner),
            run_constraints: dep_items_to_matchspec_items(&run_constraints_items.inner),
            ..Requirements::default()
        }
    }

    pub fn from_requirements(py: Python, requirements: Requirements) -> Self {
        let build: PyVecItemPackageDependency =
            matchspec_items_to_dep_items(&requirements.build).into();
        let host: PyVecItemPackageDependency =
            matchspec_items_to_dep_items(&requirements.host).into();
        let run: PyVecItemPackageDependency =
            matchspec_items_to_dep_items(&requirements.run).into();
        let run_constraints: PyVecItemPackageDependency =
            matchspec_items_to_dep_items(&requirements.run_constraints).into();

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

// --------------------------------------------------------------------------
// PyTest — wraps Item<TestType> (manual wrapper since TestType has no
// Default/Display)
// --------------------------------------------------------------------------

#[pyclass(str)]
#[derive(Clone, Serialize, Deserialize)]
pub struct PyTest {
    pub(crate) inner: Item<TestType>,
}

impl ::std::ops::Deref for PyTest {
    type Target = Item<TestType>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Item<TestType>> for PyTest {
    fn from(inner: Item<TestType>) -> Self {
        PyTest { inner }
    }
}

impl Display for PyTest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<test>")
    }
}

// --------------------------------------------------------------------------
// PyAbout — wraps About
// URL fields (homepage, documentation, repository) exposed as Option<String>
// License exposed as Option<String>
// --------------------------------------------------------------------------

#[pyclass(str)]
#[derive(Clone, Default)]
pub struct PyAbout {
    inner: About,
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
            .as_ref()
            .and_then(|v| v.as_concrete())
            .map(|u| u.to_string())
    }

    #[setter]
    pub fn set_homepage(&mut self, value: Option<String>) -> PyResult<()> {
        self.inner.homepage = match value {
            Some(s) => {
                let url: Url = s
                    .parse()
                    .map_err(|e| PyValueError::new_err(format!("invalid URL: {e}")))?;
                Some(Value::new_concrete(url, None))
            }
            None => None,
        };
        Ok(())
    }

    #[getter]
    pub fn documentation(&self) -> Option<String> {
        self.inner
            .documentation
            .as_ref()
            .and_then(|v| v.as_concrete())
            .map(|u| u.to_string())
    }

    #[setter]
    pub fn set_documentation(&mut self, value: Option<String>) -> PyResult<()> {
        self.inner.documentation = match value {
            Some(s) => {
                let url: Url = s
                    .parse()
                    .map_err(|e| PyValueError::new_err(format!("invalid URL: {e}")))?;
                Some(Value::new_concrete(url, None))
            }
            None => None,
        };
        Ok(())
    }

    #[getter]
    pub fn repository(&self) -> Option<String> {
        self.inner
            .repository
            .as_ref()
            .and_then(|v| v.as_concrete())
            .map(|u| u.to_string())
    }

    #[setter]
    pub fn set_repository(&mut self, value: Option<String>) -> PyResult<()> {
        self.inner.repository = match value {
            Some(s) => {
                let url: Url = s
                    .parse()
                    .map_err(|e| PyValueError::new_err(format!("invalid URL: {e}")))?;
                Some(Value::new_concrete(url, None))
            }
            None => None,
        };
        Ok(())
    }

    #[getter]
    pub fn license(&self) -> Option<String> {
        self.inner
            .license
            .as_ref()
            .and_then(|v| v.as_concrete())
            .map(|l| l.to_string())
    }

    #[setter]
    pub fn set_license(&mut self, value: Option<String>) -> PyResult<()> {
        self.inner.license = match value {
            Some(s) => {
                let license: License = s
                    .parse()
                    .map_err(|e| PyValueError::new_err(format!("invalid SPDX license: {e}")))?;
                Some(Value::new_concrete(license, None))
            }
            None => None,
        };
        Ok(())
    }

    #[getter]
    pub fn summary(&self) -> Option<String> {
        self.inner
            .summary
            .as_ref()
            .and_then(|v| v.as_concrete().cloned())
    }

    #[setter]
    pub fn set_summary(&mut self, value: Option<String>) {
        self.inner.summary = value.map(|s| Value::new_concrete(s, None));
    }

    #[getter]
    pub fn description(&self) -> Option<String> {
        self.inner
            .description
            .as_ref()
            .and_then(|v| v.as_concrete().cloned())
    }

    #[setter]
    pub fn set_description(&mut self, value: Option<String>) {
        self.inner.description = value.map(|s| Value::new_concrete(s, None));
    }
}

impl PyAbout {
    fn from_about(about: About) -> Self {
        PyAbout { inner: about }
    }

    fn to_about(&self) -> About {
        self.inner.clone()
    }
}

impl Display for PyAbout {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{ {} }}", self.inner)
    }
}

impl Deref for PyAbout {
    type Target = About;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// --------------------------------------------------------------------------
// PyExtra — wraps Extra
// --------------------------------------------------------------------------

#[pyclass(str)]
#[derive(Clone, Default)]
pub struct PyExtra {
    inner: Extra,
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
            // Extra in rattler-build is free-form (IndexMap<String, serde_value::Value>)
            // Try to extract recipe-maintainers if present
            if let Some(serde_value::Value::Seq(items)) = self.inner.extra.get("recipe-maintainers")
            {
                for item in items {
                    if let serde_value::Value::String(s) = item {
                        let py_item = PyItemString {
                            inner: Item::Value(Value::new_concrete(s.clone(), None)),
                        };
                        list.append(py_item)?;
                    }
                }
            }
            Ok(list.unbind())
        })
    }
}

impl PyExtra {
    fn from_extra(extra: Extra) -> Self {
        PyExtra { inner: extra }
    }

    fn to_extra(&self) -> Extra {
        self.inner.clone()
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
