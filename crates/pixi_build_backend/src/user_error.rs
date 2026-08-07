//! Marking a failure as the user's problem rather than the backend's.

use std::fmt::{Display, Formatter};

use miette::{Diagnostic, LabeledSpan, Severity, SourceCode};

/// Wraps a diagnostic to say the user's recipe, manifest or configuration is at
/// fault, not the backend.
///
/// Pixi reports these without suggesting the backend is broken, so use it for
/// anything the user can act on: a recipe that does not parse, a version that
/// does not resolve, a required field that is missing. Leave everything else
/// unwrapped -- a backend bug should read like a backend bug.
///
/// ```no_run
/// # use pixi_build_backend::user_error::UserError;
/// # fn parse() -> miette::Result<()> { Ok(()) }
/// # fn example() -> miette::Result<()> {
/// parse().map_err(UserError::new)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct UserError(miette::Report);

impl UserError {
    /// Mark a report as caused by the user's input.
    pub fn new(report: impl Into<miette::Report>) -> Self {
        Self(report.into())
    }
}

impl Display for UserError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl std::error::Error for UserError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

// Delegates to the wrapped report so that wrapping an error does not flatten
// the labels, help text or source snippets it carried.
impl Diagnostic for UserError {
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

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        self.0.diagnostic_source()
    }
}
