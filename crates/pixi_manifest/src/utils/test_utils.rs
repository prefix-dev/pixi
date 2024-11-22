use crate::toml::{ExternalWorkspaceProperties, TomlManifest};
use miette::{GraphicalReportHandler, GraphicalTheme, NamedSource, Report};

/// A helper function that generates a snapshot of the error message when
/// parsing a manifest TOML. The error is returned.
#[must_use]
pub(crate) fn expect_parse_failure(pixi_toml: &str) -> String {
    let parse_error = TomlManifest::from_toml_str(pixi_toml)
        .and_then(|manifest| manifest.into_manifests(ExternalWorkspaceProperties::default()))
        .expect_err("parsing should fail");

    // Disable colors in tests
    let mut s = String::new();
    let report_handler = GraphicalReportHandler::new()
        .with_cause_chain()
        .with_break_words(false)
        .with_theme(GraphicalTheme::unicode_nocolor());
    report_handler
        .render_report(
            &mut s,
            Report::from(parse_error)
                .with_source_code(NamedSource::new("pixi.toml", pixi_toml.to_string()))
                .as_ref(),
        )
        .unwrap();

    s
}
