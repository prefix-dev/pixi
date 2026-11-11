use std::{borrow::Borrow, fmt::Display};

use miette::{Diagnostic, LabeledSpan, Severity, SourceCode};
use pixi_compute_engine::ComputeError;
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

    /// Convert the result into a `Result<T, E>` if it is not canceled.
    fn try_into_failed(self) -> Result<Result<T, E>, CommandDispatcherError<E>>;
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

    fn try_into_failed(self) -> Result<Result<T, E>, CommandDispatcherError<E>> {
        match self {
            Ok(ok) => Ok(Ok(ok)),
            Err(CommandDispatcherError::Cancelled) => Err(CommandDispatcherError::Cancelled),
            Err(CommandDispatcherError::Failed(err)) => Ok(Err(err)),
        }
    }
}

/// Flatten a `Result<Result<T, E>, ComputeError>` returned by
/// [`ComputeEngine::with_ctx`](pixi_compute_engine::ComputeEngine::with_ctx)
/// into a [`CommandDispatcherError`]-shaped result, mapping the inner
/// domain error via `map_err`.
///
/// `ComputeError::Canceled` becomes [`CommandDispatcherError::Cancelled`].
/// `ComputeError::Cycle` is treated as `unreachable!`: every call site
/// that uses this helper runs above the cycle-detection layer, where a
/// cycle would have already been reported.
pub trait ComputeResultExt<T, E> {
    fn map_err_into_dispatcher<F>(
        self,
        map_err: impl FnOnce(E) -> F,
    ) -> Result<T, CommandDispatcherError<F>>;
}

impl<T, E> ComputeResultExt<T, E> for Result<Result<T, E>, ComputeError> {
    fn map_err_into_dispatcher<F>(
        self,
        map_err: impl FnOnce(E) -> F,
    ) -> Result<T, CommandDispatcherError<F>> {
        match self {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CommandDispatcherError::Failed(map_err(e))),
            Err(ComputeError::Cycle(c)) => {
                unreachable!("cycles should have been detected before reaching this call site, {c}")
            }
            Err(ComputeError::Canceled) => Err(CommandDispatcherError::Cancelled),
        }
    }
}

/// Flatten the doubly-wrapped result returned by
/// [`ComputeEngine::with_ctx`](pixi_compute_engine::ComputeEngine::with_ctx)
/// when the inner future already returns
/// [`Result<T, CommandDispatcherError<E>>`]. Both a
/// [`ComputeError::Canceled`] at the outer layer and a
/// [`CommandDispatcherError::Cancelled`] at the inner layer collapse
/// to [`CommandDispatcherError::Cancelled`].
pub(crate) fn flatten_with_ctx_result<T, E>(
    result: Result<Result<T, CommandDispatcherError<E>>, ComputeError>,
) -> Result<T, CommandDispatcherError<E>> {
    match result {
        Ok(inner) => inner,
        Err(ComputeError::Cycle(c)) => {
            unreachable!("cycles should have been detected before reaching this call site, {c}")
        }
        Err(ComputeError::Canceled) => Err(CommandDispatcherError::Cancelled),
    }
}

impl<E> From<simple_spawn_blocking::Cancelled> for CommandDispatcherError<E> {
    fn from(_: simple_spawn_blocking::Cancelled) -> Self {
        CommandDispatcherError::Cancelled
    }
}

impl<E: Diagnostic + 'static> Borrow<dyn Diagnostic> for Box<CommandDispatcherError<E>> {
    fn borrow(&self) -> &(dyn Diagnostic + 'static) {
        self.as_ref()
    }
}
