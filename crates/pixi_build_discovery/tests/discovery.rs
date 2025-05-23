use insta::{assert_snapshot, assert_yaml_snapshot, glob};
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use rattler_conda_types::ChannelConfig;
use std::path::{Path, PathBuf};

use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};

fn error_to_snapshot(diag: &impl Diagnostic) -> String {
    let mut report_str = String::new();
    GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .without_syntax_highlighting()
        .with_width(160)
        .render_report(&mut report_str, diag)
        .unwrap();
    report_str
}

fn discovery_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/discovery")
}

macro_rules! assert_discover_snapshot {
    ($path:expr) => {
        let file_path = Path::new(file!()).parent().unwrap();
        let channel_config = ChannelConfig::default_with_root_dir(file_path.to_owned());
        match DiscoveredBackend::discover($path, &channel_config, &EnabledProtocols::default()) {
            Ok(backend) => {
                assert_yaml_snapshot!(backend, {
                    "[\"init-params\"][\"manifest-path\"]" => insta::dynamic_redaction(|value, _path| {
                        value.as_str().unwrap().replace("\\", "/")
                     }),
                    "[\"init-params\"][\"source-dir\"]" => "[SOURCE_PATH]",
                });
            }
            Err(err) => {
                assert_snapshot!(error_to_snapshot(&err));
            }
        }
    };
}

/// A test to check what discovery looks like for different use cases.
///
/// The test cases are located in the `tests/data/discovery` directory. Every
/// directory (or subdirectory) that contains a file TEST-CASE is used as a
/// test case.
#[test]
fn test_discovery() {
    glob!("../../../tests/data/discovery", "**/TEST-CASE", |path| {
        let path = dunce::canonicalize(path.parent().unwrap()).unwrap();
        let new_suffix = insta::Settings::clone_current()
            .snapshot_suffix()
            .unwrap()
            .strip_suffix("TEST-CASE")
            .unwrap()
            .trim_end_matches(['/', '\\'])
            .to_owned();
        let source_path_regex = path.to_string_lossy().replace(r"\", r"\\");
        insta::with_settings!({
            filters => vec![
                (source_path_regex.as_str(), "file://<ROOT>"),
                (r"\\", r"/"),
            ],
            snapshot_suffix => new_suffix}, {
            assert_discover_snapshot!(&path);
        })
    });
}

#[test]
fn test_non_existing() {
    assert_discover_snapshot!(Path::new("/non-existing"));
}

#[test]
fn test_direct_recipe() {
    let path = dunce::canonicalize(discovery_directory().join("recipe_yaml/recipe.yaml")).unwrap();
    let source_path_regex = path
        .parent()
        .unwrap()
        .to_string_lossy()
        .replace(r"\", r"\\\\");
    insta::with_settings!({
        filters => vec![
            (source_path_regex.as_str(), "file://<ROOT>"),
            (r"\\", r"/"),
        ],
    }, {
        assert_discover_snapshot ! ( & path);
    });
}
