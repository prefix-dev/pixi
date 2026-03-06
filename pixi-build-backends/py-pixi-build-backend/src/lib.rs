use pyo3::prelude::*;

use crate::error::{CliException, GeneratedRecipeException};

mod cli;
pub mod error;
mod recipe_stage0;
mod types;

/// A simple macro wrapper that allows us to create a Py class
/// from an arbitrary Rust type
#[macro_export]
macro_rules! create_py_wrap {
    // Version with custom Display implementation
    ($name: ident, $type: ty, $display_impl: expr) => {
        #[pyclass(str)]
        #[derive(Clone, ::serde::Serialize, ::serde::Deserialize, Default)]
        pub struct $name {
            pub(crate) inner: $type,
        }

        impl ::std::ops::Deref for $name {
            type Target = $type;
            fn deref(&self) -> &Self::Target {
                &self.inner
            }
        }

        impl From<$type> for $name {
            fn from(inner: $type) -> Self {
                $name { inner }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let display_fn: fn(&$type, &mut std::fmt::Formatter<'_>) -> std::fmt::Result =
                    $display_impl;
                display_fn(&self.inner, f)
            }
        }
    };

    // Version with default Display implementation (delegates to inner type)
    ($name: ident, $type: ty) => {
        #[pyclass(str)]
        #[derive(Clone, ::serde::Serialize, ::serde::Deserialize, Default)]
        pub struct $name {
            pub(crate) inner: $type,
        }

        impl ::std::ops::Deref for $name {
            type Target = $type;
            fn deref(&self) -> &Self::Target {
                &self.inner
            }
        }

        impl From<$type> for $name {
            fn from(inner: $type) -> Self {
                $name { inner }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.inner)
            }
        }
    };
}

#[pymodule]
fn pixi_build_backend(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Add core types
    m.add_class::<types::PyPlatform>()?;
    m.add_class::<types::PyProjectModel>()?;
    m.add_class::<types::PyGeneratedRecipe>()?;
    m.add_class::<types::PyGenerateRecipe>()?;
    m.add_class::<types::PyPythonParams>()?;
    m.add_class::<types::PyBackendConfig>()?;
    m.add_class::<types::PyMetadataProvider>()?;

    // Add recipe_stage0 types
    m.add_class::<recipe_stage0::recipe::PyIntermediateRecipe>()?;
    m.add_class::<recipe_stage0::recipe::PyPackage>()?;
    m.add_class::<recipe_stage0::recipe::PySource>()?;
    m.add_class::<recipe_stage0::recipe::PyUrlSource>()?;
    m.add_class::<recipe_stage0::recipe::PyPathSource>()?;
    m.add_class::<recipe_stage0::recipe::PyBuild>()?;
    m.add_class::<recipe_stage0::recipe::PyScript>()?;
    m.add_class::<recipe_stage0::recipe::PyPython>()?;
    m.add_class::<recipe_stage0::recipe::PyNoArchKind>()?;
    m.add_class::<recipe_stage0::recipe::PyValueString>()?;
    m.add_class::<recipe_stage0::recipe::PyValueU64>()?;
    m.add_class::<recipe_stage0::recipe::PyConditionalRequirements>()?;
    m.add_class::<recipe_stage0::recipe::PyAbout>()?;
    m.add_class::<recipe_stage0::recipe::PyExtra>()?;

    // Add requirements types
    m.add_class::<recipe_stage0::requirements::PyPackageSpecDependencies>()?;
    m.add_class::<recipe_stage0::requirements::PyPackageDependency>()?;
    m.add_class::<recipe_stage0::requirements::PySourceMatchSpec>()?;
    m.add_class::<recipe_stage0::requirements::PySerializableMatchSpec>()?;
    m.add_class::<recipe_stage0::requirements::PySelector>()?;

    // Add conditional types
    m.add_class::<recipe_stage0::conditional::PyItemString>()?;
    m.add_class::<recipe_stage0::conditional::PyItemPackageDependency>()?;

    m.add_class::<recipe_stage0::conditional_requirements::PyVecItemPackageDependency>()?;

    m.add_class::<recipe_stage0::conditional::PyConditionalString>()?;
    m.add_class::<recipe_stage0::conditional::PyConditionalPackageDependency>()?;
    m.add_class::<recipe_stage0::conditional::PyConditionalSource>()?;

    // ListOrItem python part
    m.add_class::<recipe_stage0::conditional::PyListOrItemString>()?;
    m.add_class::<recipe_stage0::conditional::PyListOrItemPackageDependency>()?;
    m.add_class::<recipe_stage0::conditional::PyListOrItemSource>()?;

    // Add entry points
    m.add_function(wrap_pyfunction!(cli::py_main, m)?)?;
    m.add_function(wrap_pyfunction!(cli::py_main_sync, m)?)?;

    // Exceptions
    m.add("CliError", py.get_type::<CliException>())?;

    m.add(
        "GeneratedRecipeError",
        py.get_type::<GeneratedRecipeException>(),
    )?;

    Ok(())
}
