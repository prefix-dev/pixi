use std::{
    error::Error,
    fmt::{Debug, Display, Formatter},
};

use miette::{Diagnostic, LabeledSpan, Severity, SourceCode};

/// A struct that binds a [`Diagnostic`] to a [`SourceCode`]. This is similar to
/// using [`miette::Report::with_source_code`] but it retains type information.
#[derive(Debug)]
pub struct WithSourceCode<E, S> {
    pub error: E,
    pub source: S,
}

impl<E: Error + Debug, S: SourceCode + Debug> Error for WithSourceCode<E, S> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.error.source()
    }
}

impl<E: Display, S> Display for WithSourceCode<E, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl<E: Diagnostic, S: SourceCode + Debug> Diagnostic for WithSourceCode<E, S> {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.error.code()
    }

    fn severity(&self) -> Option<Severity> {
        self.error.severity()
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.error.help()
    }

    fn url<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.error.url()
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.error.source_code().or(Some(&self.source))
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.error.labels()
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        self.error.related()
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        self.error.diagnostic_source()
    }
}
