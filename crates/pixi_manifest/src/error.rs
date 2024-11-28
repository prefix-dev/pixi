use std::{
    borrow::{Borrow, Cow},
    fmt::Display,
    ops::Range,
};

use itertools::Itertools;
use miette::{Diagnostic, LabeledSpan, SourceOffset, SourceSpan};
use rattler_conda_types::{version_spec::ParseVersionSpecError, InvalidPackageNameError};
use thiserror::Error;

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

#[derive(Error, Debug, Clone)]
pub enum TomlError {
    #[error("{}", .0.message())]
    Error(toml_edit::TomlError),
    #[error("Missing table `[tool.pixi.project]`. Try running `pixi init`")]
    NoPixiTable,
    #[error("Missing field `{0}`")]
    MissingField(Cow<'static, str>, Option<Range<usize>>),
    #[error("{0}")]
    Generic(Cow<'static, str>, Option<Range<usize>>),
    #[error(transparent)]
    FeatureNotEnabled(#[from] FeatureNotEnabled),
    #[error("Could not find or access the part '{part}' in the path '[{table_name}]'")]
    TableError { part: String, table_name: String },
    #[error("Could not find or access array '{array_name}' in '[{table_name}]'")]
    ArrayError {
        array_name: String,
        table_name: String,
    },
    #[error("Could not convert pep508 to pixi pypi requirement")]
    Conversion(#[from] Box<Pep508ToPyPiRequirementError>),
    #[error(transparent)]
    InvalidNonPackageDependencies(#[from] InvalidNonPackageDependencies),
}

impl From<toml_edit::TomlError> for TomlError {
    fn from(e: toml_edit::TomlError) -> Self {
        TomlError::Error(e)
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
            feature: feature.as_str().into(),
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
        let span = match self {
            TomlError::Error(err) => err.span().map(SourceSpan::from),
            TomlError::NoPixiTable => Some(SourceSpan::new(SourceOffset::from(0), 1)),
            TomlError::Generic(_, span) | TomlError::MissingField(_, span) => {
                span.clone().map(SourceSpan::from)
            }
            TomlError::FeatureNotEnabled(err) => return err.labels(),
            TomlError::InvalidNonPackageDependencies(err) => return err.labels(),
            _ => None,
        };

        // This is here to make it easier to add more match arms in the future.
        #[allow(clippy::match_single_binding)]
        let message = match self {
            _ => None,
        };

        if let Some(span) = span {
            Some(Box::new(std::iter::once(
                LabeledSpan::new_primary_with_span(message, span),
            )))
        } else {
            None
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            TomlError::NoPixiTable => {
                Some(Box::new("Run `pixi init` to create a new project manifest"))
            }
            TomlError::FeatureNotEnabled(err) => err.help(),
            TomlError::InvalidNonPackageDependencies(err) => err.help(),
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
