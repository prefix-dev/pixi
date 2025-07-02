use std::fmt::Display;

use miette::{Diagnostic, LabeledSpan, Severity, SourceCode};
use thiserror::Error;

/// Wraps an error that might have occurred during the processing of a task.
#[derive(Debug, Clone, Error)]
pub enum CommandDispatcherError<E> {
    /// The operation was canceled.
    #[error("the operation was cancelled")]
    Cancelled,

    /// The operation failed with an error.
    #[error(transparent)]
    Failed(E),
}

impl<E: Diagnostic> Diagnostic for CommandDispatcherError<E> {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.code(),
        }
    }

    fn severity(&self) -> Option<Severity> {
        match self {
            CommandDispatcherError::Cancelled => Some(Severity::Warning),
            CommandDispatcherError::Failed(err) => err.severity(),
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.help(),
        }
    }

    fn url<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.url(),
        }
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.source_code(),
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.labels(),
        }
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.related(),
        }
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => err.diagnostic_source(),
        }
    }
}

impl<E> CommandDispatcherError<E> {
    /// Map the error of a [`CommandDispatcherError::Failed`] to another type
    /// using the provided function.
    pub fn map<U, F: FnOnce(E) -> U>(self, map: F) -> CommandDispatcherError<U> {
        match self {
            CommandDispatcherError::Cancelled => CommandDispatcherError::Cancelled,
            CommandDispatcherError::Failed(err) => CommandDispatcherError::Failed(map(err)),
        }
    }

    /// Returns `Some(E)` if the error is [`CommandDispatcherError::Failed`],
    /// otherwise `None`.
    pub fn into_failed(self) -> Option<E> {
        match self {
            CommandDispatcherError::Cancelled => None,
            CommandDispatcherError::Failed(err) => Some(err),
        }
    }
}

/// Convenience trait to make working with [`CommandDispatcherError`] type
/// easier.
pub trait CommandDispatcherErrorResultExt<T, E> {
    /// Maps the error of a [`CommandDispatcherError::Failed`] to another type
    /// using the provided function.
    fn map_err_with<U, F: FnOnce(E) -> U>(self, fun: F) -> Result<T, CommandDispatcherError<U>>;

    /// If this result is not canceled, returns the inner result type. Returns
    /// `None` if the error is [`CommandDispatcherError`].
    fn into_ok_or_failed(self) -> Option<Result<T, E>>;
}

impl<T, E> CommandDispatcherErrorResultExt<T, E> for Result<T, CommandDispatcherError<E>> {
    fn map_err_with<U, F: FnOnce(E) -> U>(self, fun: F) -> Result<T, CommandDispatcherError<U>> {
        self.map_err(|err| err.map(fun))
    }

    fn into_ok_or_failed(self) -> Option<Result<T, E>> {
        match self {
            Ok(ok) => Some(Ok(ok)),
            Err(CommandDispatcherError::Cancelled) => None,
            Err(CommandDispatcherError::Failed(err)) => Some(Err(err)),
        }
    }
}

impl<E> From<simple_spawn_blocking::Cancelled> for CommandDispatcherError<E> {
    fn from(_: simple_spawn_blocking::Cancelled) -> Self {
        CommandDispatcherError::Cancelled
    }
}
