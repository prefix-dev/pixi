use std::fmt::Display;

use miette::{Diagnostic, LabeledSpan, Severity, SourceCode};
use thiserror::Error;

/// Wraps an error that might have occurred during the processing of a task.
#[derive(Debug, Clone, Error)]
pub enum CommandQueueError<E> {
    /// The operation was canceled.
    #[error("the operation was cancelled")]
    Cancelled,

    /// The operation failed with an error.
    #[error(transparent)]
    Failed(#[from] E),
}

impl<E: Diagnostic> Diagnostic for CommandQueueError<E> {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.code(),
        }
    }

    fn severity(&self) -> Option<Severity> {
        match self {
            CommandQueueError::Cancelled => Some(Severity::Warning),
            CommandQueueError::Failed(err) => err.severity(),
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.help(),
        }
    }

    fn url<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.url(),
        }
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.source_code(),
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.labels(),
        }
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.related(),
        }
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => err.diagnostic_source(),
        }
    }
}

impl<E> CommandQueueError<E> {
    /// Map the error of a [`CommandQueueError::Failed`] to another type using
    /// the provided function.
    pub fn map<U, F: FnOnce(E) -> U>(self, map: F) -> CommandQueueError<U> {
        match self {
            CommandQueueError::Cancelled => CommandQueueError::Cancelled,
            CommandQueueError::Failed(err) => CommandQueueError::Failed(map(err)),
        }
    }

    /// Returns `Some(E)` if the error is [`CommandQueueError::Failed`],
    /// otherwise `None`.
    pub fn into_failed(self) -> Option<E> {
        match self {
            CommandQueueError::Cancelled => None,
            CommandQueueError::Failed(err) => Some(err),
        }
    }
}

/// Convenience trait to make working with [`CommandQueueError`] type easier.
pub trait CommandQueueErrorResultExt<T, E> {
    /// Maps the error of a [`CommandQueueError::Failed`] to another type using
    /// the provided function.
    fn map_err_with<U, F: FnOnce(E) -> U>(self, fun: F) -> Result<T, CommandQueueError<U>>;

    /// If this result is not canceled, returns the inner result type. Returns
    /// `None` if the error is [`CommandQueueError`].
    fn into_ok_or_failed(self) -> Option<Result<T, E>>;
}

impl<T, E> CommandQueueErrorResultExt<T, E> for Result<T, CommandQueueError<E>> {
    fn map_err_with<U, F: FnOnce(E) -> U>(self, fun: F) -> Result<T, CommandQueueError<U>> {
        self.map_err(|err| err.map(fun))
    }

    fn into_ok_or_failed(self) -> Option<Result<T, E>> {
        match self {
            Ok(ok) => Some(Ok(ok)),
            Err(CommandQueueError::Cancelled) => None,
            Err(CommandQueueError::Failed(err)) => Some(Err(err)),
        }
    }
}
