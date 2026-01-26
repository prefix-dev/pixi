use std::error::Error;

use pyo3::{PyErr, create_exception, exceptions::PyException};
use thiserror::Error;

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum PyPixiBuildBackendError {
    #[error(transparent)]
    Cli(Box<dyn Error>),

    #[error(transparent)]
    GeneratedRecipe(Box<dyn Error>),

    #[error(transparent)]
    YamlSerialization(#[from] serde_yaml::Error),
}

pub(crate) fn pretty_print_error(mut err: &dyn Error) -> String {
    let mut result = err.to_string();
    while let Some(source) = err.source() {
        result.push_str(&format!("\nCaused by: {source}"));
        err = source;
    }
    result
}

impl From<PyPixiBuildBackendError> for PyErr {
    fn from(value: PyPixiBuildBackendError) -> Self {
        match value {
            PyPixiBuildBackendError::Cli(err) => CliException::new_err(pretty_print_error(&*err)),
            PyPixiBuildBackendError::GeneratedRecipe(err) => {
                GeneratedRecipeException::new_err(pretty_print_error(&*err))
            }
            PyPixiBuildBackendError::YamlSerialization(err) => {
                YamlSerializationException::new_err(pretty_print_error(&err))
            }
        }
    }
}

create_exception!(exceptions, CliException, PyException);
create_exception!(exceptions, GeneratedRecipeException, PyException);
create_exception!(exceptions, YamlSerializationException, PyException);
