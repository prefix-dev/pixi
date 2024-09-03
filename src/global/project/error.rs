use miette::{Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource, Report};

use thiserror::Error;

/// Represents errors that can occur when working with a pixi global manifest
#[derive(Error, Debug, Clone, Diagnostic)]
pub enum ManifestError {
    #[error(transparent)]
    Error(#[from] toml_edit::TomlError),
    #[error("Could not find or access the part '{part}' in the path '[{table_name}]'")]
    TableError { part: String, table_name: String },
    #[error("Could not find or access array '{array_name}' in '[{table_name}]'")]
    ArrayError {
        array_name: String,
        table_name: String,
    },
}

impl ManifestError {
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
            ManifestError::Error(e) => e.span(),
            _ => None,
        }
    }
    fn message(&self) -> String {
        match self {
            ManifestError::Error(e) => e.message().to_owned(),
            _ => self.to_string(),
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
impl From<toml_edit::de::Error> for ManifestError {
    fn from(e: toml_edit::de::Error) -> Self {
        ManifestError::Error(e.into())
    }
}
