use crate::project::manifest::{FeatureName, TargetSelector};
use crate::project::SpecType;
use miette::{Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};
use rattler_conda_types::{InvalidPackageNameError, ParseMatchSpecError};
use thiserror::Error;

/// An error that is returned when a certain spec is missing.
#[derive(Debug, Error, Diagnostic)]
#[error("{name} is missing")]
pub struct SpecIsMissing {
    // The name of the dependency that is missing,
    pub name: String,

    // The type of the dependency that is missing.
    pub spec_type: SpecType,

    /// Whether the dependency itself is missing or the entire dependency spec type is missing
    pub spec_type_is_missing: bool,

    // The target from which the dependency is missing.
    pub target: Option<TargetSelector>,

    // The feature from which the dependency is missing.
    pub feature: Option<FeatureName>,
}

impl SpecIsMissing {
    /// Constructs a new `SpecIsMissing` error that indicates that a spec type is missing.
    ///
    /// This is constructed for instance when the `[build-dependencies]` section is missing.
    pub fn spec_type_is_missing(name: impl Into<String>, spec_type: SpecType) -> Self {
        Self {
            name: name.into(),
            spec_type,
            spec_type_is_missing: true,
            target: None,
            feature: None,
        }
    }

    /// Constructs a new `SpecIsMissing` error that indicates that a spec is missing
    pub fn dep_is_missing(name: impl Into<String>, spec_type: SpecType) -> Self {
        Self {
            name: name.into(),
            spec_type,
            spec_type_is_missing: false,
            target: None,
            feature: None,
        }
    }

    /// Set the target from which the spec is missing.
    pub fn with_target(mut self, target: TargetSelector) -> Self {
        self.target = Some(target);
        self
    }

    /// Sets the feature from which the spec is missing.
    pub fn with_feature(mut self, feature: FeatureName) -> Self {
        self.feature = Some(feature);
        self
    }
}

#[derive(Error, Debug)]
pub enum RequirementConversionError {
    #[error("Invalid package name error")]
    InvalidPackageNameError(#[from] InvalidPackageNameError),
    #[error("Failed to parse specification")]
    ParseError(#[from] ParseMatchSpecError),
    #[error("Error converting requirement from pypi to conda")]
    Unimplemented,
}

#[derive(Error, Debug, Clone)]
pub enum TomlError {
    #[error("{0}")]
    Error(#[from] toml_edit::TomlError),
    #[error("Missing field `project`")]
    NoProjectTable,
    #[error("Missing field `name`")]
    NoProjectName(Option<std::ops::Range<usize>>),
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
        }
    }
    fn message(&self) -> &str {
        match self {
            TomlError::Error(e) => e.message(),
            TomlError::NoProjectTable => "Missing field `project`",
            TomlError::NoProjectName(_) => "Missing field `name`",
        }
    }
}
impl From<toml_edit::de::Error> for TomlError {
    fn from(e: toml_edit::de::Error) -> Self {
        TomlError::Error(e.into())
    }
}
