use std::{
    error::Error,
    fmt::{Display, Formatter},
};

use jsonrpsee::types::ErrorObject;
use miette::{Diagnostic, Severity};
use serde::{Deserialize, Deserializer};

#[derive(Debug)]
pub struct BackendError {
    message: String,
    source: Option<Box<BackendErrorCause>>,
    severity: Severity,
}

#[derive(Debug, Diagnostic)]
pub struct BackendErrorCause {
    message: String,
    cause: Option<Box<BackendErrorCause>>,
}

impl Display for BackendErrorCause {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.message)
    }
}

impl Error for BackendErrorCause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
            .as_ref()
            .map(|e| e.as_ref() as &(dyn Error + 'static))
    }
}

impl<'e> From<ErrorObject<'e>> for BackendError {
    fn from(value: ErrorObject<'e>) -> Self {
        // Try to parse the error contained in the data field.
        let error: RawErrorValue = value
            .data()
            .and_then(|value| serde_json::from_str(value.get()).ok())
            .unwrap_or_default();

        let error_data = value.data().map(|b| b.get()).unwrap_or("");
        dbg!(error_data);

        let source = error.causes.0.into_iter().fold(None, |previous, cause| {
            Some(Box::new(BackendErrorCause {
                message: cause,
                cause: previous,
            }))
        });

        Self {
            message: value.message().to_owned(),
            source,
            severity: error.severity.0,
        }
    }
}

impl Display for BackendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.message)
    }
}

impl Error for BackendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(|e| e.as_ref() as &dyn Error)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawErrorValue {
    severity: OrDefault<Severity>,
    causes: OrDefault<Vec<String>>,
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

impl Diagnostic for BackendError {
    fn severity(&self) -> Option<Severity> {
        Some(self.severity)
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        self.source.as_ref().map(|e| e.as_ref() as &dyn Diagnostic)
    }
}
