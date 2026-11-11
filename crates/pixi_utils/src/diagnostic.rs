//! Helpers for typed [`Diagnostic`] errors.

use std::{
    error::Error,
    fmt::{self, Debug, Display},
};

use miette::{Diagnostic, LabeledSpan, Severity, SourceCode};

/// The cause of a typed error that adds a header to another diagnostic, the
/// typed equivalent of miette's `wrap_err`.
///
/// Use it as the `#[source]` field of a variant marked
/// `#[diagnostic(forward(..))]`. The variant then shows the cause's code, help
/// and labels at the top level, and the cause and its own causes are listed
/// through [`Error::source`]. Forwarding to the cause directly would also
/// forward its `diagnostic_source`, which skips the cause's own message and
/// nests the next diagnostic under it.
///
/// The cause is boxed, which keeps the wrapping error small.
pub struct Cause<E>(Box<E>);

impl<E> Cause<E> {
    pub fn new(error: E) -> Self {
        Self(Box::new(error))
    }
}

impl<E: Debug> Debug for Cause<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<E: Display> Display for Cause<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<E: Error> Error for Cause<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

impl<E: Diagnostic> Diagnostic for Cause<E> {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.0.code()
    }

    fn severity(&self) -> Option<Severity> {
        self.0.severity()
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.0.help()
    }

    fn url<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.0.url()
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.0.source_code()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.0.labels()
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        self.0.related()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error, Diagnostic)]
    #[error("inner")]
    struct Inner;

    #[derive(Debug, thiserror::Error, Diagnostic)]
    #[error("middle")]
    #[diagnostic(code(middle), help("middle help"))]
    struct Middle {
        #[source]
        #[diagnostic_source]
        source: Inner,
    }

    #[derive(Debug, thiserror::Error, Diagnostic)]
    #[error("outer")]
    #[diagnostic(forward(source))]
    struct Outer {
        #[source]
        source: Cause<Middle>,
    }

    /// Like `wrap_err`, the header takes the cause's code and help, and the
    /// cause stays in the chain even when it has a diagnostic source.
    #[test]
    fn forwards_everything_but_the_diagnostic_source() {
        let outer = Outer {
            source: Cause::new(Middle { source: Inner }),
        };
        assert_eq!(outer.code().unwrap().to_string(), "middle");
        assert_eq!(outer.help().unwrap().to_string(), "middle help");
        assert!(outer.diagnostic_source().is_none());
        let middle = outer.source().unwrap();
        assert_eq!(middle.to_string(), "middle");
        assert_eq!(middle.source().unwrap().to_string(), "inner");
    }
}
