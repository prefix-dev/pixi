use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use insta::{assert_snapshot, assert_yaml_snapshot, glob};
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use rattler_conda_types::ChannelConfig;

fn discovery_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/discovery")
}

fn redact_path(value: &str) -> String {
    let path = PathBuf::from(value);
    let skipped = path
        .components()
        .skip_while(|c| *c != std::path::Component::Normal(OsStr::new("discovery")))
        .skip(1)
        .collect::<PathBuf>();
    let display_skipped = skipped.display();
    let str_skipped = display_skipped.to_string();
    let prettified_norm = str_skipped.replace(r"\\", r"/").replace(r"\", r"/");
    let prettified = prettified_norm.trim_end_matches(['/', '\\']);
    format!("file://<ROOT>/{}", prettified)
}

/// This macro is used to assert the discovery of a backend and compare it with a snapshot.
///
/// Errors are also handled and compared with a snapshot.
macro_rules! assert_discover_snapshot {
    ($path:expr) => {
        let file_path = Path::new(file!()).parent().unwrap();
        let channel_config = ChannelConfig::default_with_root_dir(file_path.to_owned());

        // Run the discovery on the input path
        match DiscoveredBackend::discover($path, &channel_config, &EnabledProtocols::default()) {
            Ok(backend) => {
                assert_yaml_snapshot!(backend,
                // Perform some redaction on fields that contain paths. We need to make them cross-platform compatible.
                {
                    "[\"init-params\"][\"manifest-path\"]" => insta::dynamic_redaction(|value, _path| {
                        redact_path(value.as_str().unwrap())
                     }),
                    "[\"init-params\"][\"workspace-root\"]" => insta::dynamic_redaction(|value, _path| {
                        redact_path(value.as_str().unwrap())
                     }),
                    "[\"init-params\"][\"source-anchor\"]" => insta::dynamic_redaction(|value, _path| {
                        redact_path(value.as_str().unwrap())
                     }),
                });
            }
            Err(err) => {
                assert_snapshot!(pixi_test_utils::format_diagnostic(&err));
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
