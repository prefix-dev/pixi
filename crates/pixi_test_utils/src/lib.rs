use itertools::Itertools;
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, NamedSource, Report};
use std::path::Path;

/// Format a TOML parse error into a string that can be used to generate
/// snapshots.
pub fn format_parse_error(source: &str, error: impl Into<Report>) -> String {
    format_diagnostic(
        error
            .into()
            .with_source_code(NamedSource::new("pixi.toml", source.to_string()))
            .as_ref(),
    )
}

/// Format a diagnostic into a string that can be used to generate snapshots.
pub fn format_diagnostic(error: &dyn Diagnostic) -> String {
    // Disable colors in tests
    let mut s = String::new();
    let report_handler = GraphicalReportHandler::new()
        .with_cause_chain()
        .with_break_words(false)
        .with_theme(GraphicalTheme::unicode_nocolor());
    report_handler.render_report(&mut s, error).unwrap();

    // Strip machine specific paths
    let cargo_root = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();
    s = s.replace(&cargo_root, "<CARGO_ROOT>");

    // Replace backslashes with forward slashes
    s = s.replace("\\", "/");

    // Remove trailing whitespace in the error message.
    s.lines()
        .map(|line| line.trim_end())
        .format("\n")
        .to_string()
}
