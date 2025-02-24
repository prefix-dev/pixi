use std::{
    borrow::{Borrow, Cow},
    fmt::{Display, Formatter},
    ops::Range,
};

use itertools::Itertools;
use miette::{Diagnostic, LabeledSpan, SourceOffset, SourceSpan};
use rattler_conda_types::{version_spec::ParseVersionSpecError, InvalidPackageNameError};
use thiserror::Error;
use toml_span::{DeserError, Error};

use super::pypi::pypi_requirement::Pep508ToPyPiRequirementError;
use crate::{KnownPreviewFeature, WorkspaceManifest};

#[derive(Error, Debug, Clone, Diagnostic)]
pub enum DependencyError {
    #[error("{} is already a dependency.", .0)]
    Duplicate(String),
    #[error("spec type {} is missing.", .0)]
    NoSpecType(String),
    #[error("dependency {} is missing.", .0)]
    NoDependency(String),
    #[error("No Pypi dependencies.")]
    NoPyPiDependencies,
    #[error(transparent)]
    Pep508ToPyPiRequirementError(#[from] Box<Pep508ToPyPiRequirementError>),
}

#[derive(Error, Debug)]
pub enum RequirementConversionError {
    #[error("Invalid package name error")]
    InvalidPackageNameError(#[from] InvalidPackageNameError),
    #[error("Failed to parse version")]
    InvalidVersion(#[from] ParseVersionSpecError),
}

#[derive(Default, Debug)]
pub struct GenericError {
    pub message: Cow<'static, str>,
    pub span: Option<Range<usize>>,
    pub span_label: Option<Cow<'static, str>>,
    pub labels: Vec<LabeledSpan>,
    pub help: Option<Cow<'static, str>>,
}

impl GenericError {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            ..Default::default()
        }
    }

    pub fn with_span(mut self, span: Range<usize>) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_span_label(mut self, span: impl Into<Cow<'static, str>>) -> Self {
        self.span_label = Some(span.into());
        self
    }

    pub fn with_opt_span(mut self, span: Option<Range<usize>>) -> Self {
        self.span = span;
        self
    }

    pub fn with_label(mut self, label: LabeledSpan) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_labels(mut self, labels: impl IntoIterator<Item = LabeledSpan>) -> Self {
        self.labels.extend(labels);
        self
    }

    pub fn with_opt_label(mut self, text: impl Into<String>, range: Option<Range<usize>>) -> Self {
        if let Some(range) = range {
            self.labels.push(LabeledSpan::new_with_span(
                Some(text.into()),
                SourceSpan::from(range),
            ));
        }
        self
    }

    pub fn with_help(mut self, help: impl Into<Cow<'static, str>>) -> Self {
        self.help = Some(help.into());
        self
    }
}

#[derive(Error, Debug)]
pub enum TomlError {
    Error(toml_edit::TomlError),
    TomlError(toml_span::Error),
    NoPixiTable,
    MissingField(Cow<'static, str>, Option<Range<usize>>),
    Generic(GenericError),
    #[error(transparent)]
    FeatureNotEnabled(#[from] FeatureNotEnabled),
    TableError {
        part: String,
        table_name: String,
    },
    ArrayError {
        array_name: String,
        table_name: String,
    },
    #[error(transparent)]
    Conversion(#[from] Box<Pep508ToPyPiRequirementError>),
    #[error(transparent)]
    InvalidNonPackageDependencies(#[from] InvalidNonPackageDependencies),
}

impl From<toml_span::Error> for TomlError {
    fn from(value: Error) -> Self {
        TomlError::TomlError(value)
    }
}

impl From<GenericError> for TomlError {
    fn from(value: GenericError) -> Self {
        TomlError::Generic(value)
    }
}

impl Display for TomlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TomlError::Error(err) => write!(f, "{}", err.message()),
            TomlError::TomlError(err) => match &err.kind {
                toml_span::ErrorKind::UnexpectedKeys { expected, .. } => {
                    write!(
                        f,
                        "Unexpected keys, expected only {}",
                        expected
                            .iter()
                            .format_with(", ", |key, f| f(&format_args!("'{}'", key)))
                    )
                }
                toml_span::ErrorKind::UnexpectedValue { expected, .. } => {
                    write!(
                        f,
                        "Expected one of {}",
                        expected
                            .iter()
                            .format_with(", ", |key, f| f(&format_args!("'{}'", key)))
                    )
                }
                _ => write!(f, "{}", err),
            },
            TomlError::NoPixiTable => write!(f, "Missing table `[tool.pixi.project]`"),
            TomlError::MissingField(key, _) => write!(f, "Missing field `{key}`"),
            TomlError::Generic(err) => write!(f, "{}", &err.message),
            TomlError::FeatureNotEnabled(err) => write!(f, "{err}"),
            TomlError::TableError { part, table_name } => write!(
                f,
                "Could not find or access the part '{part}' in the path '[{table_name}]'"
            ),
            TomlError::ArrayError {
                array_name,
                table_name,
            } => write!(
                f,
                "Could not find or access array '{array_name}' in '[{table_name}]'"
            ),
            TomlError::Conversion(_) => {
                write!(f, "Could not convert pep508 to pixi pypi requirement")
            }
            TomlError::InvalidNonPackageDependencies(err) => write!(f, "{err}"),
        }
    }
}

impl From<toml_edit::TomlError> for TomlError {
    fn from(e: toml_edit::TomlError) -> Self {
        TomlError::Error(e)
    }
}

impl From<DeserError> for TomlError {
    fn from(mut value: DeserError) -> Self {
        // TODO: Now we only take the first error, but we could make this smarter
        TomlError::TomlError(value.errors.remove(0))
    }
}

#[derive(Error, Debug, Clone)]
#[error("{message}")]
pub struct FeatureNotEnabled {
    pub feature: Cow<'static, str>,
    pub message: Cow<'static, str>,
    pub span: Option<std::ops::Range<usize>>,
}

impl FeatureNotEnabled {
    pub fn new(message: impl Into<Cow<'static, str>>, feature: KnownPreviewFeature) -> Self {
        Self {
            feature: <&'static str>::from(feature).into(),
            message: message.into(),
            span: None,
        }
    }

    pub fn with_opt_span(self, span: Option<std::ops::Range<usize>>) -> Self {
        Self { span, ..self }
    }
}

impl Diagnostic for FeatureNotEnabled {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        Some(Box::new(format!(
            "Add `preview = [\"{}\"]` under [workspace] to enable the preview feature",
            self.feature
        )))
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        if let Some(span) = self.span.clone() {
            Some(Box::new(std::iter::once(
                LabeledSpan::new_primary_with_span(None, span),
            )))
        } else {
            None
        }
    }
}

impl Diagnostic for TomlError {
    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let mut additional_spans = None;
        let span = match self {
            TomlError::Error(err) => err.span().map(SourceSpan::from),
            TomlError::Generic(GenericError { span, labels, .. }) => {
                additional_spans = Some(labels.clone());
                if labels.iter().all(|label| !label.primary()) {
                    span.clone().map(SourceSpan::from)
                } else {
                    None
                }
            }
            TomlError::TomlError(toml_span::Error { kind, span, .. }) => match kind {
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
                _ => Some(SourceSpan::new(span.start.into(), span.end - span.start)),
            },
            TomlError::NoPixiTable => Some(SourceSpan::new(SourceOffset::from(0), 1)),
            TomlError::MissingField(_, span) => span.clone().map(SourceSpan::from),
            TomlError::FeatureNotEnabled(err) => return err.labels(),
            TomlError::InvalidNonPackageDependencies(err) => return err.labels(),
            _ => None,
        };

        // This is here to make it easier to add more match arms in the future.
        #[allow(clippy::match_single_binding)]
        let message = match self {
            TomlError::TomlError(toml_span::Error {
                kind: toml_span::ErrorKind::Deprecated { new, .. },
                ..
            }) => Some(format!("replace this with '{}'", new)),
            TomlError::Generic(GenericError { span_label, .. }) => {
                span_label.clone().map(Cow::into_owned)
            }
            _ => None,
        };

        if let Some(span) = span {
            Some(Box::new(
                std::iter::once(LabeledSpan::new_primary_with_span(message, span))
                    .chain(additional_spans.into_iter().flatten()),
            ))
        } else if let Some(additional_spans) = additional_spans {
            Some(Box::new(additional_spans.into_iter()))
        } else {
            None
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            TomlError::NoPixiTable => {
                Some(Box::new("Run `pixi init` to create a new project manifest"))
            }
            TomlError::TomlError(toml_span::Error { kind, .. }) => match kind {
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
            },
            TomlError::FeatureNotEnabled(err) => err.help(),
            TomlError::InvalidNonPackageDependencies(err) => err.help(),
            TomlError::Generic(GenericError { help, .. }) => help
                .as_deref()
                .map(|str| Box::new(str.to_string()) as Box<dyn Display>),
            _ => None,
        }
    }
}

impl TomlError {
    pub fn table_error(part: &str, table_name: &str) -> Self {
        Self::TableError {
            part: part.into(),
            table_name: table_name.into(),
        }
    }

    pub fn array_error(array_name: &str, table_name: &str) -> Self {
        Self::ArrayError {
            array_name: array_name.into(),
            table_name: table_name.into(),
        }
    }
}
impl From<toml_edit::de::Error> for TomlError {
    fn from(e: toml_edit::de::Error) -> Self {
        TomlError::Error(e.into())
    }
}

/// Error for when a feature is not defined in the project manifest.
#[derive(Debug, Error)]
pub struct UnknownFeature {
    feature: String,
    existing_features: Vec<String>,
}

impl UnknownFeature {
    pub fn new(feature: String, manifest: impl Borrow<WorkspaceManifest>) -> Self {
        // Find the top 2 features that are closest to the feature name.
        let existing_features = manifest
            .borrow()
            .features
            .keys()
            .filter_map(|f| {
                let distance = strsim::jaro(f.as_str(), &feature);
                (distance > 0.6).then_some((distance, f))
            })
            .sorted_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, name)| name.to_string())
            .take(2)
            .collect();
        Self {
            feature,
            existing_features,
        }
    }
}

impl std::fmt::Display for UnknownFeature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the feature '{}' is not defined in the project manifest",
            self.feature
        )
    }
}

impl miette::Diagnostic for UnknownFeature {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        if !self.existing_features.is_empty() {
            Some(Box::new(format!(
                "Did you mean '{}'?",
                self.existing_features.join("' or '")
            )))
        } else {
            None
        }
    }
}

/// An error that indicates that some package sections are only valid when the
/// manifest describes a package instead of a workspace.
#[derive(Debug, Error, Clone)]
#[error("build-, host- and run-dependency sections are only valid for packages.")]
pub struct InvalidNonPackageDependencies {
    pub invalid_dependency_sections: Vec<Range<usize>>,
}

impl Diagnostic for InvalidNonPackageDependencies {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        Some(Box::new(
            "These sections are only valid when the manifest describes a package instead of a workspace.\nAdd a `[package]` section to the manifest to fix this error or remove the offending sections.",
        ))
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(self.invalid_dependency_sections.iter().map(
            |range| LabeledSpan::new_with_span(None, SourceSpan::from(range.clone())),
        )))
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("an error occurred while parsing the manifest")]
pub struct MultiTomlError {
    #[related]
    pub errors: Vec<TomlError>,
}

impl From<DeserError> for MultiTomlError {
    fn from(value: DeserError) -> Self {
        Self {
            errors: value.errors.into_iter().map(Into::into).collect(),
        }
    }
}
