use itertools::Itertools;
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, NamedSource, Report};
use pixi_consts::consts::PIXI_VERSION;
use std::path::{Path, PathBuf};

pub mod git_fixture;
pub mod mock_repo_data;

pub use git_fixture::GitRepoFixture;
pub use mock_repo_data::{
    LocalChannel, MockRepoData, Package, PackageBuilder, create_conda_package,
};

/// Resolves a sibling workspace binary built next to the current test
/// executable.
///
/// The test binary lives in `<target>/<...>/<profile>/deps/`, so the
/// requested binary is expected one directory up. Deriving the location from
/// [`std::env::current_exe`] rather than a hard-coded path keeps this working
/// under a custom `CARGO_TARGET_DIR` (the workspace sets `target/pixi`) and
/// any cross-compilation target-triple subdirectory.
///
/// Panics if the binary cannot be found, since a missing binary means the
/// workspace was not built with `--all-targets`.
pub fn workspace_bin(name: &str) -> PathBuf {
    let current_exe = std::env::current_exe().expect("failed to resolve current executable");
    let bin_dir = current_exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|p| p.join("release"))
        .expect("test executable has no parent directory");
    let bin_path = bin_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        bin_path.is_file(),
        "could not find workspace binary `{name}` at `{}`. \
         Build the workspace with `--all-targets` so the binary is available.",
        bin_path.display()
    );
    bin_path
}

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

    // Replace pixi version with a placeholder so snapshots don't need updating on releases
    s = s.replace(PIXI_VERSION, "<PIXI_VERSION>");

    // Replace backslashes with forward slashes
    s = s.replace("\\", "/");

    // Remove trailing whitespace in the error message.
    s.lines()
        .map(|line| line.trim_end())
        .format("\n")
        .to_string()
}
