use itertools::Itertools;
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, NamedSource, Report};
use pixi_consts::consts::PIXI_VERSION;
use std::path::Path;

pub mod git_fixture;
pub mod mock_repo_data;

pub use git_fixture::{GitRepoFixture, git_lfs_available};
pub use mock_repo_data::{
    LocalChannel, MockRepoData, Package, PackageBuilder, create_conda_package,
};

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

    // Error messages can embed styled text (e.g. `fancy_display`), which emits
    // ANSI escape sequences whenever `console` thinks the terminal supports
    // colors. Strip them so snapshots don't depend on the environment the tests
    // run in.
    s = console::strip_ansi_codes(&s).into_owned();

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

/// Builds a record of `name` and `version` that was uploaded on 2020-01-01,
/// to check which records an exclude-newer configuration excludes.
pub fn uploaded_record(
    name: &str,
    version: &str,
    channel: Option<&rattler_conda_types::ChannelUrl>,
) -> rattler_conda_types::RepoDataRecord {
    use std::str::FromStr;

    let mut package_record = rattler_conda_types::PackageRecord::new(
        rattler_conda_types::PackageName::new_unchecked(name),
        rattler_conda_types::Version::from_str(version).unwrap(),
        "0".to_string(),
    );
    package_record.timestamp = Some(
        "2020-01-01T00:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .into(),
    );
    let file_name = format!("{name}-{version}-0.conda");
    rattler_conda_types::RepoDataRecord {
        package_record,
        identifier: rattler_conda_types::package::DistArchiveIdentifier::from_str(&file_name)
            .unwrap(),
        url: match channel {
            Some(channel) => channel.url().join(&format!("noarch/{file_name}")).unwrap(),
            None => url::Url::parse("https://example.com/pkg.conda").unwrap(),
        },
        channel: channel.map(ToString::to_string),
    }
}
