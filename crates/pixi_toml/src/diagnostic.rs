use itertools::Itertools;
use miette::{LabeledSpan, SourceSpan};
use std::fmt::{Display, Formatter};
use toml_span::Error;

/// A wrapper around [`toml_span::Error`] that implements the `miette::Diagnostic` trait.
#[derive(Debug)]
pub struct TomlDiagnostic(pub toml_span::Error);

impl std::error::Error for TomlDiagnostic {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<toml_span::Error> for TomlDiagnostic {
    fn from(value: Error) -> Self {
        Self(value)
    }
}

impl Display for TomlDiagnostic {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0.kind {
            toml_span::ErrorKind::UnexpectedKeys { expected, .. } => {
                write!(
                    f,
                    "Unexpected keys, expected only {}",
                    expected
                        .iter()
                        .format_with(", ", |key, f| f(&format_args!("'{key}'")))
                )
            }
            toml_span::ErrorKind::UnexpectedValue { expected, .. } => {
                write!(
                    f,
                    "Expected one of {}",
                    expected
                        .iter()
                        .format_with(", ", |key, f| f(&format_args!("'{key}'")))
                )
            }
            _ => write!(f, "{}", &self.0),
        }
    }
}

impl miette::Diagnostic for TomlDiagnostic {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        let toml_span::Error { kind, .. } = &self.0;
        match kind {
            toml_span::ErrorKind::UnexpectedValue { expected, value } => {
                if let Some(value) = value {
                    if let Some((_, similar)) = expected
                        .iter()
                        .filter_map(|expected| {
                            let distance = strsim::jaro(expected, value);
                            (distance > 0.6).then_some((distance, expected))
                        })
                        .max_by(|(a, _), (b, _)| {
                            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                        })
                    {
                        return Some(Box::new(format!("Did you mean '{similar}'?")));
                    }
                }
                None
            }
            toml_span::ErrorKind::UnexpectedKeys { expected, keys } => {
                if let Ok((single, _)) = keys.iter().exactly_one() {
                    if let Some((_, similar)) = expected
                        .iter()
                        .filter_map(|expected| {
                            let distance = strsim::jaro(expected, single);
                            (distance > 0.6).then_some((distance, expected))
                        })
                        .max_by(|(a, _), (b, _)| {
                            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                        })
                    {
                        return Some(Box::new(format!("Did you mean '{similar}'?")));
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let toml_span::Error { kind, span, .. } = &self.0;
        let primary_span = match kind {
            toml_span::ErrorKind::UnexpectedKeys { keys, .. } => {
                let mut labels = Vec::new();
                for (key, span) in keys {
                    labels.push(LabeledSpan::new_with_span(
                        Some(format!("'{key}' was not expected here")),
                        SourceSpan::new(span.start.into(), span.end - span.start),
                    ));
                }
                return Some(Box::new(labels.into_iter()));
            }
            toml_span::ErrorKind::DuplicateKey { first, .. } => {
                let labels = vec![
                    LabeledSpan::new_primary_with_span(
                        Some("duplicate defined here".to_string()),
                        SourceSpan::new(span.start.into(), span.end - span.start),
                    ),
                    LabeledSpan::new_with_span(
                        Some("first defined here".to_string()),
                        SourceSpan::new(first.start.into(), first.end - first.start),
                    ),
                ];
                return Some(Box::new(labels.into_iter()));
            }
            _ => SourceSpan::new(span.start.into(), span.end - span.start),
        };

        let message = if let toml_span::ErrorKind::Deprecated { new, .. } = kind {
            Some(format!("replace this with '{new}'"))
        } else {
            None
        };

        Some(Box::new(std::iter::once(
            LabeledSpan::new_primary_with_span(message, primary_span),
        )))
    }
}
