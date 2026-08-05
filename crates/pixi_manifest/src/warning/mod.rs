mod deprecation;

use std::{fmt::Display, sync::Arc};

pub use deprecation::Deprecation;
use miette::{Diagnostic, LabeledSpan, NamedSource, SourceSpan};
use thiserror::Error;

use crate::{error::GenericError, utils::WithSourceCode};

#[derive(Debug, Error, Diagnostic)]
pub enum Warning {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Deprecation(#[from] Deprecation),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Generic(#[from] GenericWarning),

    /// A feature is defined in the manifest but not used in any environment.
    /// Kept as a separate variant so commands that are about to change
    /// environment membership can filter it out.
    #[error(transparent)]
    #[diagnostic(transparent)]
    UnusedFeature(GenericWarning),
}

impl Warning {
    pub fn unused_feature(error: GenericError) -> Self {
        Warning::UnusedFeature(GenericWarning { error })
    }

    pub fn is_unused_feature(&self) -> bool {
        matches!(self, Warning::UnusedFeature(_))
    }

    pub fn is_deprecation(&self) -> bool {
        matches!(self, Warning::Deprecation(_))
    }
}

impl From<GenericError> for Warning {
    fn from(error: GenericError) -> Self {
        GenericWarning { error }.into()
    }
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

#[derive(Debug, Error)]
#[error("{}", error.message)]
pub struct GenericWarning {
    error: GenericError,
}

impl Diagnostic for GenericWarning {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        None
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Warning)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.error
            .help
            .as_deref()
            .map(|str| Box::new(str) as Box<dyn Display>)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let span = if self.error.labels.iter().all(|label| !label.primary()) {
            self.error.span.clone().map(SourceSpan::from)
        } else {
            None
        };

        if let Some(span) = span {
            Some(Box::new(
                std::iter::once(LabeledSpan::new_primary_with_span(
                    self.error.span_label.as_deref().map(str::to_string),
                    span,
                ))
                .chain(self.error.labels.clone()),
            ))
        } else if !self.error.labels.is_empty() {
            Some(Box::new(self.error.labels.iter().cloned()))
        } else {
            None
        }
    }
}
