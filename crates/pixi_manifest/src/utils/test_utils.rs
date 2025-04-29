use crate::toml::ExternalWorkspaceProperties;
use crate::toml::{FromTomlStr, TomlManifest};
use itertools::Itertools;
use pixi_test_utils::format_parse_error;

/// A helper function that generates a snapshot of the error message when
/// parsing a manifest TOML. The error is returned.
#[must_use]
pub(crate) fn expect_parse_failure(pixi_toml: &str) -> String {
    let parse_error = TomlManifest::from_toml_str(pixi_toml)
        .and_then(|manifest| {
            manifest.into_workspace_manifest(ExternalWorkspaceProperties::default(), None)
        })
        .expect_err("parsing should fail");

    format_parse_error(pixi_toml, parse_error)
}

/// A helper function that generates a snapshot of the warnings message when
/// parsing a manifest TOML. The error is returned.
#[must_use]
pub(crate) fn expect_parse_warnings(pixi_toml: &str) -> String {
    match <TomlManifest as FromTomlStr>::from_toml_str(pixi_toml).and_then(|manifest| {
        manifest.into_workspace_manifest(ExternalWorkspaceProperties::default(), None)
    }) {
        Ok((_, _, warnings)) => warnings
            .into_iter()
            .map(|warning| format_parse_error(pixi_toml, warning))
            .join("\n\n"),
        Err(err) => format_parse_error(pixi_toml, err),
    }
}
