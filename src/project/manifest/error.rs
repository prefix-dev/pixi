use miette::{IntoDiagnostic, LabeledSpan, NamedSource, Report};
use rattler_conda_types::{InvalidPackageNameError, ParseMatchSpecError};
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum DependencyError {
    #[error("{} is already a dependency.", console::style(.0).bold())]
    Duplicate(String),
    #[error("Spec type {} is missing.", console::style(.0).bold())]
    NoSpecType(String),
    #[error("Dependency {} is missing.", console::style(.0).bold())]
    NoDependency(String),
}

#[derive(Error, Debug)]
pub enum RequirementConversionError {
    #[error("Invalid package name error")]
    InvalidPackageNameError(#[from] InvalidPackageNameError),
    #[error("Failed to parse specification")]
    ParseError(#[from] ParseMatchSpecError),
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
