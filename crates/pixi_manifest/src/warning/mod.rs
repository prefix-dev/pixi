mod deprecation;

use crate::utils::WithSourceCode;
pub use deprecation::Deprecation;
use miette::{Diagnostic, NamedSource};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum Warning {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Deprecation(#[from] Deprecation),
}

#[derive(Debug)]
pub struct WithWarnings<T, W = Warning> {
    pub value: T,
    pub warnings: Vec<W>,
}

impl<T, W> WithWarnings<T, W> {
    pub fn with_warnings(self, warnings: Vec<W>) -> Self {
        Self { warnings, ..self }
    }
}

impl<T, W> From<T> for WithWarnings<T, W> {
    fn from(value: T) -> Self {
        Self {
            value,
            warnings: Vec::new(),
        }
    }
}

pub type WarningWithSource = WithSourceCode<Warning, NamedSource<Arc<str>>>;
