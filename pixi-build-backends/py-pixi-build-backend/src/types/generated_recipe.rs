use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::{
    create_py_wrap,
    error::pretty_print_error,
    recipe_stage0::recipe::PyIntermediateRecipe,
    types::metadata_provider::get_input_globs_from_provider,
    types::{PyBackendConfig, PyMetadataProvider, PyPlatform, PyProjectModel, PyPythonParams},
};
use miette::IntoDiagnostic;
use pixi_build_backend::generated_recipe::{
    DefaultMetadataProvider, GenerateRecipe, GeneratedRecipe, PythonParams,
};
use pixi_build_backend::{NormalizedKey, Variable};
use pixi_build_types::ProjectModel;
use pyo3::{
    Py, PyAny, PyErr, PyResult, Python,
    exceptions::PyRuntimeError,
    pyclass, pymethods,
    types::{PyAnyMethods, PyList, PyString},
};
use rattler_conda_types::{ChannelUrl, Platform};
use recipe_stage0::recipe::IntermediateRecipe;

create_py_wrap!(PyVecString, Vec<String>, |v: &Vec<String>,
                                           f: &mut std::fmt::Formatter<
    '_,
>| {
    write!(f, "[{}]", v.join(", "))
});

#[pyclass(get_all, set_all)]
#[derive(Clone)]
pub struct PyGeneratedRecipe {
    pub(crate) recipe: Py<PyIntermediateRecipe>,
    pub(crate) metadata_input_globs: Py<PyVecString>,
    pub(crate) build_input_globs: Py<PyVecString>,
}

#[pymethods]
impl PyGeneratedRecipe {
    #[new]
    pub fn new(py: Python) -> PyResult<Self> {
        Ok(PyGeneratedRecipe {
            recipe: Py::new(py, PyIntermediateRecipe::new(py)?)?,
            metadata_input_globs: Py::new(py, PyVecString::default())?,
            build_input_globs: Py::new(py, PyVecString::default())?,
        })
    }

    #[staticmethod]
    pub fn from_model(py: Python, model: PyProjectModel) -> PyResult<Self> {
        let generated_recipe =
            GeneratedRecipe::from_model(model.inner.clone(), &mut DefaultMetadataProvider)
                .map_err(|e| PyErr::new::<PyRuntimeError, _>(pretty_print_error(&e)))?;

        let py_recipe = Py::new(
            py,
            PyIntermediateRecipe::from_intermediate_recipe(generated_recipe.recipe, py),
        )?;
        let py_metadata_globs = Py::new(
            py,
            PyVecString::from(
                generated_recipe
                    .metadata_input_globs
                    .into_iter()
                    .collect::<Vec<String>>(),
            ),
        )?;
        let py_build_globs = Py::new(
            py,
            PyVecString::from(
                generated_recipe
                    .build_input_globs
                    .into_iter()
                    .collect::<Vec<String>>(),
            ),
        )?;

        Ok(PyGeneratedRecipe {
            recipe: py_recipe,
            metadata_input_globs: py_metadata_globs,
            build_input_globs: py_build_globs,
        })
    }

    #[staticmethod]
    pub fn from_model_with_provider(
        py: Python,
        model: PyProjectModel,
        metadata_provider: Py<PyAny>,
    ) -> PyResult<Self> {
        let mut provider = PyMetadataProvider::new(metadata_provider.clone());
        let generated_recipe = GeneratedRecipe::from_model(model.inner.clone(), &mut provider)
            .map_err(|e| PyErr::new::<PyRuntimeError, _>(pretty_print_error(&e)))?;

        // Get additional input globs from the metadata provider if available
        let mut metadata_input_globs = generated_recipe.metadata_input_globs;
        let provider_globs = get_input_globs_from_provider(&metadata_provider);
        metadata_input_globs.extend(provider_globs);

        let py_recipe = Py::new(
            py,
            PyIntermediateRecipe::from_intermediate_recipe(generated_recipe.recipe, py),
        )?;
        let py_metadata_globs = Py::new(
            py,
            PyVecString::from(metadata_input_globs.into_iter().collect::<Vec<String>>()),
        )?;
        let py_build_globs = Py::new(
            py,
            PyVecString::from(
                generated_recipe
                    .build_input_globs
                    .into_iter()
                    .collect::<Vec<String>>(),
            ),
        )?;

        Ok(PyGeneratedRecipe {
            recipe: py_recipe,
            metadata_input_globs: py_metadata_globs,
            build_input_globs: py_build_globs,
        })
    }
}

impl PyGeneratedRecipe {
    pub fn to_generated_recipe(&self, py: Python) -> GeneratedRecipe {
        let recipe: IntermediateRecipe = self.recipe.borrow(py).to_intermediate_recipe(py);
        let metadata_input_globs: BTreeSet<String> =
            (*self.metadata_input_globs.borrow(py).clone())
                .clone()
                .into_iter()
                .collect();
        let build_input_globs: BTreeSet<String> = (*self.build_input_globs.borrow(py).clone())
            .clone()
            .into_iter()
            .collect();

        GeneratedRecipe {
            recipe,
            metadata_input_globs,
            build_input_globs,
        }
    }
}

/// Trait part
#[pyclass]
#[derive(Clone)]
pub struct PyGenerateRecipe {
    model: Py<PyAny>,
}

#[pymethods]
impl PyGenerateRecipe {
    #[new]
    pub fn new(model: Py<PyAny>) -> Self {
        PyGenerateRecipe { model }
    }
}

#[async_trait::async_trait]
impl GenerateRecipe for PyGenerateRecipe {
    type Config = PyBackendConfig;

    async fn generate_recipe(
        &self,
        model: &ProjectModel,
        config: &Self::Config,
        manifest_root: PathBuf,
        host_platform: Platform,
        python_params: Option<PythonParams>,
        _variants: &HashSet<NormalizedKey>,
        channels: Vec<ChannelUrl>,
        _cache_dir: Option<PathBuf>,
    ) -> miette::Result<GeneratedRecipe> {
        let recipe: GeneratedRecipe = Python::attach(|py| {
            let manifest_str = manifest_root.to_string_lossy().to_string();

            // we don't pass the wrapper but the python inner model directly
            let py_object = config.model.clone();

            // For other types, we try to wrap them into the Python class
            // So user can use the Python API
            let project_model_class = py
                .import("pixi_build_backend.types.project_model")
                .into_diagnostic()?
                .getattr("ProjectModel")
                .into_diagnostic()?;

            let project_model = project_model_class
                .call_method1("_from_py", (PyProjectModel::from(model),))
                .into_diagnostic()?;

            let platform_model_class = py
                .import("pixi_build_backend.types.platform")
                .into_diagnostic()?
                .getattr("Platform")
                .into_diagnostic()?;

            let platform_model = platform_model_class
                .call_method1("_from_py", (PyPlatform::from(host_platform),))
                .into_diagnostic()?;

            let python_params_class = py
                .import("pixi_build_backend.types.python_params")
                .into_diagnostic()?
                .getattr("PythonParams")
                .into_diagnostic()?;
            let python_params_model = python_params_class
                .call_method1(
                    "_from_py",
                    (PyPythonParams::from(python_params.unwrap_or_default()),),
                )
                .into_diagnostic()?;

            // Convert channels to Python list of strings
            let channels_list =
                PyList::new(py, channels.iter().map(|c| c.to_string())).into_diagnostic()?;

            let generated_recipe_py = self
                .model
                .bind(py)
                .call_method(
                    "generate_recipe",
                    (
                        project_model,
                        py_object,
                        PyString::new(py, manifest_str.as_str()),
                        platform_model,
                        python_params_model,
                        channels_list,
                    ),
                    None,
                )
                .into_diagnostic()?;

            // To expose a nice API for the user, we extract the PyGeneratedRecipe
            // calling private _into_py method
            let generated_recipe: PyGeneratedRecipe = generated_recipe_py
                .call_method0("_into_py")
                .into_diagnostic()?
                .extract::<PyGeneratedRecipe>()
                .into_diagnostic()?;

            Ok::<_, miette::Report>(generated_recipe.to_generated_recipe(py))
        })?;

        Ok(recipe)
    }

    /// Returns a list of globs that should be used to find the input files
    /// for the build process.
    /// For example, this could be a list of source files or configuration files
    /// used by Cmake.
    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        workdir: impl AsRef<Path>,
        editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
        Python::attach(|py| {
            let workdir = workdir.as_ref();

            // we don't pass the wrapper but the python inner model directly
            let py_object = config.model.clone();

            let input_globs = self
                .model
                .bind(py)
                .call_method(
                    "extract_input_globs_from_build",
                    (py_object, workdir, editable),
                    None,
                )
                .into_diagnostic()?
                .extract::<Vec<String>>()
                .into_diagnostic()?
                .into_iter()
                .collect::<BTreeSet<String>>();
            Ok::<_, miette::Report>(input_globs)
        })
    }

    fn default_variants(
        &self,
        host_platform: Platform,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        Python::attach(|py| {
            let variants_dict = self
                .model
                .bind(py)
                .call_method("default_variants", (PyPlatform::from(host_platform),), None)
                .into_diagnostic()?
                .extract::<BTreeMap<String, Vec<String>>>()
                .into_diagnostic()?;

            let mut variants = BTreeMap::new();
            for (key, values) in variants_dict {
                variants.insert(
                    NormalizedKey::from(key),
                    values.into_iter().map(Variable::from).collect(),
                );
            }
            Ok::<_, miette::Report>(variants)
        })
    }
}
