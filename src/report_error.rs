use ariadne::{Report, Source};
use std::{
    fmt::{Debug, Display, Formatter},
    ops::Range,
};

/// An error that contains a [`ariadne::Report`]. This allows the application to display a very
/// nicely formatted diagnostic.
#[derive(Debug)]
pub struct ReportError {
    pub report: Report<'static, (&'static str, Range<usize>)>,
    pub source: (&'static str, Source),
}

impl std::error::Error for ReportError {}

impl Display for ReportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.report.fmt(f)
    }
}

impl ReportError {
    pub fn eprint(self) {
        self.report.eprint(self.source).unwrap()
    }
}
