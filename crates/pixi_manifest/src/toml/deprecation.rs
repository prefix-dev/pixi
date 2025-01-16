use std::{borrow::Cow, fmt::Display};

use miette::{Diagnostic, LabeledSpan, Severity, SourceSpan};
use thiserror::Error;
use toml_span::Span;

/// A deprecation message for a field.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct Deprecation {
    pub message: Cow<'static, str>,
    pub labels: Vec<LabeledSpan>,
    pub help: Option<Cow<'static, str>>,
}

impl Deprecation {
    pub fn renamed_field(old_name: &str, new_name: &str, span: Span) -> Self {
        Self {
            message: format!(
                "The `{}` field is deprecated. Use `{}` instead.",
                old_name, new_name
            )
            .into(),
            labels: vec![LabeledSpan::new_primary_with_span(
                Some(format!("replace this with '{}'", new_name)),
                SourceSpan::new(span.start.into(), span.end - span.start),
            )],
            help: None,
        }
    }
}

impl Diagnostic for Deprecation {
    fn severity(&self) -> Option<Severity> {
        Some(Severity::Warning)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.help.as_ref().map(|h| Box::new(h) as Box<dyn Display>)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(self.labels.iter().cloned()))
    }
}
