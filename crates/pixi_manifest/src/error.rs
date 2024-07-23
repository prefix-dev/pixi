use std::{borrow::Borrow, fmt::Display};

use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use rattler_conda_types::{InvalidPackageNameError, ParseMatchSpecError};
use thiserror::Error;

use super::pypi::pypi_requirement::Pep508ToPyPiRequirementError;
use crate::ProjectManifest;

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
}

#[derive(Error, Debug)]
pub enum RequirementConversionError {
    #[error("Invalid package name error")]
    InvalidPackageNameError(#[from] InvalidPackageNameError),
    #[error("Failed to parse specification")]
    ParseError(#[from] ParseMatchSpecError),
}

#[derive(Error, Debug, Clone, Diagnostic)]
pub enum TomlError {
    #[error("{0}")]
    Error(#[from] toml_edit::TomlError),
    #[error("Missing field `project`")]
    NoProjectTable,
    #[error("Missing field `name`")]
    NoProjectName(Option<std::ops::Range<usize>>),
    #[error("Could not find or access the part '{part}' in the path '[{table_name}]'")]
    TableError { part: String, table_name: String },
    #[error("Could not find or access array '{array_name}' in '[{table_name}]'")]
    ArrayError {
        array_name: String,
        table_name: String,
    },
    #[error("Could not convert pep508 to pixi pypi requirement")]
    Conversion(#[from] Pep508ToPyPiRequirementError),
}

impl TomlError {
    pub fn to_fancy<T>(&self, file_name: &str, contents: impl Into<String>) -> Result<T, Report> {
        if let Some(span) = self.span() {
            Err(miette::miette!(
                labels = vec![LabeledSpan::at(span, self.message())],
                "failed to parse project manifest"
            )
            .with_source_code(NamedSource::new(file_name, contents.into())))
        } else {
            Err(self.clone()).into_diagnostic()
        }
    }

    fn span(&self) -> Option<std::ops::Range<usize>> {
        match self {
            TomlError::Error(e) => e.span(),
            TomlError::NoProjectTable => Some(0..1),
            TomlError::NoProjectName(span) => span.clone(),
            _ => None,
        }
    }
    fn message(&self) -> &str {
        match self {
            TomlError::Error(e) => e.message(),
            TomlError::NoProjectTable => "Missing field `project`",
            TomlError::NoProjectName(_) => "Missing field `name`",
            _ => "",
        }
    }

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
    pub fn new(feature: String, manifest: impl Borrow<ProjectManifest>) -> Self {
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
