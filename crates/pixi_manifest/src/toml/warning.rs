use crate::toml::deprecation::Deprecation;
use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum Warning {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Deprecation(#[from] Deprecation),
}

#[derive(Debug)]
pub struct WithWarnings<T> {
    pub value: T,
    pub warnings: Vec<Warning>,
}

impl<T> WithWarnings<T> {
    pub fn with_warnings(self, warnings: Vec<Warning>) -> Self {
        Self { warnings, ..self }
    }
}

impl<T> From<T> for WithWarnings<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            warnings: Vec::new(),
        }
    }
}
