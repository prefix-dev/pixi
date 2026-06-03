use std::{borrow::Cow, fmt::Display, ops::Range};

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
            message: format!("The `{old_name}` field is deprecated. Use `{new_name}` instead.")
                .into(),
            labels: vec![LabeledSpan::new_primary_with_span(
                Some(format!("replace this with '{new_name}'")),
                SourceSpan::new(span.start.into(), span.end - span.start),
            )],
            help: None,
        }
    }

    /// Deprecation of the legacy `[package.target.*]` dependency tables in
    /// favor of `if(<expression>)` conditional dependency tables. `help` carries
    /// the tailored replacement suggestion.
    pub fn package_target(help: String, span: Option<Range<usize>>) -> Self {
        let labels = span
            .map(|span| {
                vec![LabeledSpan::new_primary_with_span(
                    Some("deprecated target selector".to_string()),
                    SourceSpan::new(span.start.into(), span.end - span.start),
                )]
            })
            .unwrap_or_default();
        Self {
            message:
                "the `[package.target]` tables are deprecated in favor of conditional dependencies"
                    .into(),
            labels,
            help: Some(help.into()),
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
