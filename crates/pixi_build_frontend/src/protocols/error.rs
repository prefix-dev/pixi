use std::{
    error::Error,
    fmt::{Display, Formatter},
    path::Path,
};

use jsonrpsee::types::ErrorObject;
use miette::{Diagnostic, LabeledSpan, NamedSource, Severity, SourceCode};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Default)]
pub struct BackendError {
    code: Option<String>,
    message: String,
    severity: Severity,
    cause: Option<Box<BackendError>>,
    labels: Vec<LabeledSpan>,
    related: Vec<BackendError>,
    source: Option<NamedSource<String>>,
    help: Option<String>,
    url: Option<String>,
}

impl Display for BackendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.message)
    }
}

impl Error for BackendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_ref().map(|e| e.as_ref() as &dyn Error)
    }
}

impl Diagnostic for BackendError {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.code.as_ref().map(|c| Box::new(c) as Box<dyn Display>)
    }

    fn severity(&self) -> Option<Severity> {
        Some(self.severity)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.help.as_ref().map(|h| Box::new(h) as Box<dyn Display>)
    }

    fn url<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.url.as_ref().map(|u| Box::new(u) as Box<dyn Display>)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.source.as_ref().map(|s| s as &dyn SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        if self.labels.is_empty() {
            None
        } else {
            Some(Box::new(self.labels.iter().cloned()))
        }
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        self.cause.as_ref().map(|e| e.as_ref() as &dyn Diagnostic)
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        if self.related.is_empty() {
            None
        } else {
            Some(Box::new(self.related.iter().map(|e| e as &dyn Diagnostic)))
        }
    }
}

impl BackendError {
    pub fn from_json_rpc(error: ErrorObject<'_>, root_directory: &Path) -> Self {
        // Try to parse the error contained in the data field.
        let error: RawErrorValue = error
            .data()
            .and_then(|value| serde_json::from_str(value.get()).ok())
            .unwrap_or_default();

        Self::from_raw_error(error, root_directory)
    }

    fn from_raw_error(error: RawErrorValue, root_directory: &Path) -> Self {
        let RawErrorValue {
            code,
            message,
            severity,
            causes,
            related,
            labels,
            help,
            url,
            filename,
        } = error;

        // Recursively build the cause chain.
        let cause = causes.0.into_iter().fold(None, |previous, cause| {
            Some(Box::new(BackendError {
                message: cause,
                cause: previous,
                ..BackendError::default()
            }))
        });

        // See if we can determine the source of the error.
        let source = filename.and_then(|filename| {
            let filename = filename.0;
            if filename.is_empty() {
                return None;
            }

            let path = Path::new(&filename);
            let absolute_path = if path.is_absolute() {
                path
            } else {
                &root_directory.join(path)
            };

            match fs_err::read_to_string(absolute_path) {
                Ok(source) => Some(NamedSource::new(&filename, source)),
                Err(_) => None,
            }
        });

        let labels = labels
            .0
            .into_iter()
            .map(|label| LabeledSpan::new(label.label, label.span.offset, label.span.length))
            .collect();

        let related = related
            .0
            .into_iter()
            .map(|related| Self::from_raw_error(related, root_directory))
            .collect();

        Self {
            code: code.map(|code| code.0),
            message: message.0,
            severity: severity.0,
            cause,
            labels,
            related,
            source,
            help: help.map(|help| help.0),
            url: url.map(|url| url.0),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawErrorValue {
    code: Option<OrDefault<String>>,
    message: OrDefault<String>,
    severity: OrDefault<Severity>,
    causes: OrDefault<Vec<String>>,
    related: OrDefault<Vec<RawErrorValue>>,
    labels: OrDefault<Vec<RawLabeledSpan>>,
    help: Option<OrDefault<String>>,
    url: Option<OrDefault<String>>,
    filename: Option<OrDefault<String>>,
}

#[derive(Debug, Deserialize)]
struct RawLabeledSpan {
    label: Option<String>,
    span: RawSourceSpan,
}

#[derive(Debug, Deserialize)]
struct RawSourceSpan {
    offset: usize,
    length: usize,
}

#[derive(Debug, Default)]
struct OrDefault<T: Default>(T);

impl<'de, T: Default + Deserialize<'de>> Deserialize<'de> for OrDefault<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d)
            .or_else(|_| Ok(T::default()))
            .map(OrDefault)
    }
}
