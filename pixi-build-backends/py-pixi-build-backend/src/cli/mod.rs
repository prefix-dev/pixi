use std::sync::Arc;

use pixi_build_backend::{cli_main, intermediate_backend::IntermediateBackendInstantiator};
use pyo3::{Bound, PyAny, PyResult, Python, pyfunction};
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::runtime::Runtime;

use crate::error::PyPixiBuildBackendError;
use crate::types::PyGenerateRecipe;

#[pyfunction]
pub fn py_main(
    py: Python<'_>,
    generator: PyGenerateRecipe,
    args: Vec<String>,
) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let generator = Arc::new(generator);
        cli_main(
            |log| IntermediateBackendInstantiator::<PyGenerateRecipe>::new(log, generator),
            args,
        )
        .await
        .map_err(|e| PyPixiBuildBackendError::Cli(e.into()))?;
        Ok(())
    })
}

#[pyfunction]
pub fn py_main_sync(generator: PyGenerateRecipe, args: Vec<String>) -> PyResult<()> {
    // Create the runtime
    let rt = Runtime::new().unwrap();
    rt.block_on(async move {
        let generator = Arc::new(generator);
        cli_main(
            |log| IntermediateBackendInstantiator::<PyGenerateRecipe>::new(log, generator),
            args,
        )
        .await
        .map_err(|e| PyPixiBuildBackendError::Cli(e.into()))?;
        Ok(())
    })
}
