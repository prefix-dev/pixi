use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::NamedSource;
use toml_edit::DocumentMut;

use super::{
    CondaScriptError, CondaScriptMetadata,
    envelope::{self, CLOSING_MARKER, OPENING_MARKER},
    error::{EnvelopeError, EnvelopeErrorKind, MetadataError},
};
use crate::script::block::{LineEnding, serialize_block};

/// A code file containing a `conda-script` metadata block.
#[derive(Debug, Clone)]
pub struct CondaScriptManifest {
    path: PathBuf,
    metadata: CondaScriptMetadata,
    toml: String,
    prefix: String,
    prelude: String,
    postlude: String,
    line_ending: LineEnding,
}

impl CondaScriptManifest {
    /// Read the `conda-script` block from a file.
    ///
    /// Returns `Ok(None)` when the file contains no block.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Option<Self>, CondaScriptError> {
        let contents = fs_err::read(&path)?;
        Self::from_source(path, &contents)
    }

    /// Parse a `conda-script` block from source at a given path.
    ///
    /// The path is only used for diagnostics and to locate the script later;
    /// this function never reads it.
    pub fn from_source(
        path: impl AsRef<Path>,
        contents: &[u8],
    ) -> Result<Option<Self>, CondaScriptError> {
        // A quick byte scan keeps files without a block out of the UTF-8
        // requirement: only a file mentioning the marker must decode.
        if !contents
            .windows(OPENING_MARKER.len())
            .any(|window| window == OPENING_MARKER.as_bytes())
        {
            return Ok(None);
        }

        let path = std::path::absolute(path)?;
        let source: Arc<str> = Arc::from(std::str::from_utf8(contents)?);
        let source_name = path.to_string_lossy().into_owned();

        let block = match envelope::parse_block(&source) {
            Ok(Some(block)) => block,
            Ok(None) => return Ok(None),
            Err(kind) => {
                return Err(Box::new(EnvelopeError {
                    kind,
                    source: NamedSource::new(source_name, source),
                })
                .into());
            }
        };

        let metadata = match CondaScriptMetadata::from_toml_str(&block.metadata) {
            Ok(metadata) => metadata,
            Err(mut errors) => {
                return Err(Box::new(MetadataError {
                    error: errors.errors.remove(0).into(),
                    source: NamedSource::new(source_name, source),
                    source_map: block.source_map,
                })
                .into());
            }
        };

        Ok(Some(Self {
            path,
            metadata,
            toml: block.metadata,
            prefix: block.prefix,
            prelude: block.prelude,
            postlude: block.postlude,
            line_ending: LineEnding::detect(contents),
        }))
    }

    /// Reads the `conda-script` block of a script file, tolerating a
    /// malformed block in a Python file.
    ///
    /// Returns `Ok(None)` when the file has no block or when a malformed
    /// block appears in a Python file, so the caller falls back to the PEP
    /// 723 path: a Python script may contain an accidental line ending in
    /// the opening marker, say inside an indented docstring, and must keep
    /// working as it did before the conda-script format existed. When
    /// `surface_errors` is set or the file cannot be a PEP 723 script
    /// anyway, a block error is reported instead.
    pub fn detect_with_fallback(
        path: &Path,
        surface_errors: bool,
    ) -> Result<Option<Self>, CondaScriptError> {
        match Self::from_path(path) {
            Ok(manifest) => Ok(manifest),
            Err(error @ CondaScriptError::Io(_)) => Err(error),
            Err(error) => {
                let is_python = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("py")
                            || extension.eq_ignore_ascii_case("pyw")
                    });
                // A file with both block kinds is not a stray marker but a
                // real conflict; editing or running it as PEP 723 would
                // maintain a file `pixi run` refuses.
                let both_kinds = matches!(
                    &error,
                    CondaScriptError::Envelope(envelope)
                        if matches!(envelope.kind, EnvelopeErrorKind::BothBlockKinds { .. })
                );
                if surface_errors || !is_python || both_kinds {
                    Err(error)
                } else {
                    tracing::debug!(
                        "ignoring a malformed conda-script block in {}: {error}",
                        path.display()
                    );
                    Ok(None)
                }
            }
        }
    }

    /// The absolute path of the script file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The parsed block content.
    pub fn metadata(&self) -> &CondaScriptMetadata {
        &self.metadata
    }

    /// The raw TOML text of the block, without comment prefixes.
    pub fn toml(&self) -> &str {
        &self.toml
    }

    /// The block content as an editable TOML document.
    pub fn metadata_document(&self) -> Result<DocumentMut, toml_edit::TomlError> {
        self.toml.parse()
    }

    /// The full file contents with the block replaced by `metadata`.
    ///
    /// The code around the block and the comment prefix stay untouched.
    pub fn render_metadata(&self, metadata: &DocumentMut) -> String {
        let block = serialize_block(
            &metadata.to_string(),
            &self.prefix,
            &format!("{}{OPENING_MARKER}", self.prefix),
            &format!("{}{CLOSING_MARKER}", self.prefix),
            self.line_ending.as_str(),
        );
        format!("{}{}{}", self.prelude, block, self.postlude)
    }

    /// Replace the metadata block while preserving the code around it.
    pub fn write_metadata(&self, metadata: &DocumentMut) -> Result<(), CondaScriptError> {
        fs_err::write(&self.path, self.render_metadata(metadata))?;
        Ok(())
    }

    /// Renders the metadata as a `pixi.toml` document.
    ///
    /// The dependency tables are spliced in verbatim from the block, so the
    /// specs reach the manifest parser exactly as written. `[dependencies]`
    /// and `[tool.pixi.pypi-dependencies]` fill the default feature while
    /// `[tool.pixi.dependencies]` becomes a separate feature of the default
    /// environment, which merges the two spec sets the way pixi merges
    /// features: both specs of a package reach the solver.
    pub fn synthetic_manifest(&self) -> Result<String, toml_edit::TomlError> {
        let mut block: toml_edit::DocumentMut = self.toml.parse()?;
        let channels = block
            .remove("channels")
            .expect("`channels` is required by the metadata model");
        let dependencies = block.remove("dependencies");
        let mut pixi = block
            .get_mut("tool")
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|tool| tool.get_mut("pixi"))
            .and_then(toml_edit::Item::as_table_like_mut);
        let pixi_dependencies = pixi.as_mut().and_then(|pixi| pixi.remove("dependencies"));
        let pixi_pypi_dependencies = pixi
            .as_mut()
            .and_then(|pixi| pixi.remove("pypi-dependencies"));

        let name = self
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("script");

        let mut document = toml_edit::DocumentMut::new();
        let mut workspace = toml_edit::Table::new();
        workspace.insert("name", toml_edit::value(name));
        workspace.insert("channels", channels);
        workspace.insert(
            "platforms",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
        document.insert("workspace", toml_edit::Item::Table(workspace));

        if let Some(dependencies) = dependencies {
            document.insert("dependencies", dependencies);
        }

        if let Some(pixi_pypi_dependencies) = pixi_pypi_dependencies {
            document.insert("pypi-dependencies", pixi_pypi_dependencies);
        }

        if pixi_dependencies.is_some() {
            let mut tool_pixi = toml_edit::Table::new();
            tool_pixi.set_implicit(true);
            if let Some(pixi_dependencies) = pixi_dependencies {
                tool_pixi.insert("dependencies", pixi_dependencies);
            }
            let mut feature = toml_edit::Table::new();
            feature.set_implicit(true);
            feature.insert("tool-pixi", toml_edit::Item::Table(tool_pixi));
            document.insert("feature", toml_edit::Item::Table(feature));

            let mut default = toml_edit::Array::new();
            default.push("tool-pixi");
            let mut environments = toml_edit::Table::new();
            environments.insert(
                "default",
                toml_edit::Item::Value(toml_edit::Value::Array(default)),
            );
            document.insert("environments", toml_edit::Item::Table(environments));
        }

        Ok(document.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_pypi_spec::PypiPackageName;
    use pixi_test_utils::format_diagnostic;
    use rattler_conda_types::PackageName;

    use super::super::Entrypoint;
    use super::*;

    /// A name for the source in diagnostics; `from_source` is given the
    /// contents directly and never reads this path.
    ///
    /// It points inside the crate because `from_source` absolutizes the path:
    /// `format_diagnostic` rewrites the crate root to `<CARGO_ROOT>` before it
    /// normalizes separators, so the snapshots hold on Windows too.
    fn example_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("example.c")
    }

    fn parse(contents: &str) -> Result<Option<CondaScriptManifest>, CondaScriptError> {
        CondaScriptManifest::from_source(example_path(), contents.as_bytes())
    }

    fn parse_error(contents: &str) -> String {
        format_diagnostic(&parse(contents).unwrap_err())
    }

    #[test]
    fn parses_a_c_style_block() {
        let manifest = parse(
            r#"// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "gcc -o ${CACHE}/main ${SCRIPT} -lz && ${CACHE}/main"
//
// [dependencies]
// gcc = "*"
// zlib = { version = "1.3.*", when = "__unix" }
// /// end-conda-script
#include <zlib.h>
"#,
        )
        .unwrap()
        .unwrap();

        let metadata = manifest.metadata();
        assert_eq!(
            metadata
                .channels
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["conda-forge"]
        );
        assert!(matches!(
            &metadata.entrypoint,
            Entrypoint::Uniform(command)
                if command == "gcc -o ${CACHE}/main ${SCRIPT} -lz && ${CACHE}/main"
        ));
        insta::assert_snapshot!(
            metadata
                .dependencies
                .iter()
                .map(|(name, spec)| format!("{} = {spec:?}", name.as_normalized()))
                .collect::<Vec<_>>()
                .join("\n"),
            @r#"
        gcc = Version(Any)
        zlib = DetailedVersion(DetailedSpec { version: Some(StrictRange(StartsWith, StrictVersion(Version { version: [[0], [1], [3]], local: [] }))), build: None, build_number: None, file_name: None, extras: None, flags: None, channel: None, subdir: None, license: None, license_family: None, condition: Some(MatchSpec(MatchSpec { name: Exact(PackageName { normalized: None, source: "__unix" }), version: None, build: None, build_number: None, file_name: None, extras: None, flags: None, channel: None, subdir: None, namespace: None, md5: None, sha256: None, url: None, license: None, license_family: None, condition: None, track_features: None })), track_features: None, md5: None, sha256: None })
        "#
        );
        assert!(
            manifest
                .toml()
                .starts_with("channels = [\"conda-forge\"]\n")
        );
    }

    #[test]
    fn recognizes_odd_comment_prefixes() {
        for (prefix, name) in [
            ("-- ", "Lua"),
            ("; ", "Lisp"),
            ("%% ", "MATLAB"),
            ("#\t", "tabbed"),
        ] {
            let contents = format!(
                "{prefix}/// conda-script\n{prefix}channels = [\"conda-forge\"]\n{prefix}entrypoint = \"run ${{SCRIPT}}\"\n{prefix}/// end-conda-script\n"
            );
            let manifest = parse(&contents)
                .unwrap_or_else(|error| panic!("{name} block failed: {error}"))
                .unwrap_or_else(|| panic!("{name} block was not recognized"));
            assert_eq!(manifest.metadata().channels.len(), 1);
        }
    }

    #[test]
    fn a_prefix_with_alphanumerics_or_nothing_is_not_an_opening() {
        // A mention of the marker inside code must not open a block.
        let embedded = parse("const char *marker = \"// /// conda-script\";\n").unwrap();
        assert!(embedded.is_none());

        // The prefix must be non-empty: a bare marker line opens nothing.
        let bare = parse("/// conda-script\nchannels = []\n/// end-conda-script\n").unwrap();
        assert!(bare.is_none());
    }

    #[test]
    fn a_bare_prefix_line_is_an_empty_metadata_line() {
        let manifest = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n#\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            manifest.toml(),
            "channels = [\"conda-forge\"]\n\nentrypoint = \"python ${SCRIPT}\"\n"
        );
    }

    #[test]
    fn parses_crlf_line_endings() {
        let contents = "# /// conda-script\r\n# channels = [\"conda-forge\"]\r\n# entrypoint = \"python ${SCRIPT}\"\r\n# /// end-conda-script\r\nprint()\r\n";
        let manifest = parse(contents).unwrap().unwrap();
        assert_eq!(manifest.metadata().channels.len(), 1);
    }

    #[test]
    fn parses_a_block_behind_a_utf8_bom() {
        let contents = "\u{feff}# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n";
        let manifest = parse(contents).unwrap().unwrap();
        assert_eq!(manifest.metadata().channels.len(), 1);
    }

    #[test]
    fn parses_toml_1_1_multiline_inline_tables() {
        let manifest = parse(
            r#"// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "python ${SCRIPT}"
//
// [dependencies]
// pytorch = {
//   version = ">=2.4",
//   build = "*cuda*",
// }
// /// end-conda-script
"#,
        )
        .unwrap()
        .unwrap();
        assert!(
            manifest
                .metadata()
                .dependencies
                .contains_key(&PackageName::new_unchecked("pytorch"))
        );
    }

    #[test]
    fn parses_the_tool_pixi_table_and_ignores_foreign_tools() {
        let manifest = parse(
            r#"# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "python ${SCRIPT}"
#
# [dependencies]
# python = "3.13.*"
#
# [tool.pixi.dependencies]
# simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git" }
#
# [tool.pixi.pypi-dependencies]
# requests = ">=2"
#
# [tool.some-future-runner]
# option = { anything = "goes" }
# /// end-conda-script
"#,
        )
        .unwrap()
        .unwrap();

        let pixi = &manifest.metadata().pixi;
        let simple_app = pixi
            .dependencies
            .get(&PackageName::new_unchecked("simple-app"))
            .unwrap();
        assert!(simple_app.is_source());
        assert!(
            pixi.pypi_dependencies
                .contains_key(&PypiPackageName::from_str("requests").unwrap())
        );
    }

    #[test]
    fn an_empty_dependency_table_means_any_version() {
        let manifest = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# python = {}\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();
        insta::assert_snapshot!(
            format!("{:?}", manifest.metadata().dependencies[&PackageName::new_unchecked("python")]),
            @"Version(Any)"
        );
    }

    #[test]
    fn a_file_without_a_block_is_not_a_conda_script() {
        assert!(parse("print('hello')\n").unwrap().is_none());
        // A file without the marker never has to be valid UTF-8.
        assert!(
            CondaScriptManifest::from_source(example_path(), &[0xff, 0xfe, 0x00])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reads_a_block_from_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.c");
        fs_err::write(
            &path,
            "// /// conda-script\n// channels = [\"conda-forge\"]\n// entrypoint = \"run ${SCRIPT}\"\n// /// end-conda-script\n",
        )
        .unwrap();

        let manifest = CondaScriptManifest::from_path(&path).unwrap().unwrap();
        assert_eq!(manifest.path(), path);

        let empty = directory.path().join("empty.c");
        fs_err::write(&empty, "int main(void) { return 0; }\n").unwrap();
        assert!(CondaScriptManifest::from_path(&empty).unwrap().is_none());
    }

    #[test]
    fn renders_a_synthetic_pixi_manifest() {
        let manifest = parse(
            r#"// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "python ${SCRIPT}"
//
// [dependencies]
// python = "3.13.*"
// simple-app = "0.1.*"
// gcc = { version = "*", when = "__unix" }
// pytorch = {
//   version = ">=2.4",
//   build = "*cuda*",
// }
//
// [tool.pixi.dependencies]
// simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git" }
//
// [tool.pixi.pypi-dependencies]
// requests = ">=2"
// /// end-conda-script
"#,
        )
        .unwrap()
        .unwrap();

        insta::assert_snapshot!(manifest.synthetic_manifest().unwrap(), @r##"
        [workspace]
        name = "example"
        channels = ["conda-forge"]
        platforms = []

        [dependencies]
        python = "3.13.*"
        simple-app = "0.1.*"
        gcc = { version = "*", when = "__unix" }
        pytorch = {
          version = ">=2.4",
          build = "*cuda*",
        }

        [feature.tool-pixi.dependencies]
        simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git" }

        [environments]
        default = ["tool-pixi"]

        [pypi-dependencies]
        requests = ">=2"
        "##);
    }

    #[test]
    fn a_synthetic_manifest_without_tool_pixi_has_no_feature() {
        let manifest = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();

        insta::assert_snapshot!(manifest.synthetic_manifest().unwrap(), @r#"
        [workspace]
        name = "example"
        channels = ["conda-forge"]
        platforms = []
        "#);
    }

    #[test]
    fn errors_on_an_unterminated_block() {
        insta::assert_snapshot!(parse_error(
            "// /// conda-script\n// channels = [\"conda-forge\"]\n"
        ), @r#"
         × the `/// conda-script` block has no closing `/// end-conda-script` marker
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:1:1]
        1 │ // /// conda-script
          · ─────────┬─────────
          ·          ╰── the block opens here
        2 │ // channels = ["conda-forge"]
          ╰────
         help: close the block with `// /// end-conda-script`
        "#);
    }

    #[test]
    fn errors_on_a_line_without_the_prefix() {
        insta::assert_snapshot!(parse_error(
            "// /// conda-script\n// channels = [\"conda-forge\"]\nint main(void) {}\n// /// end-conda-script\n"
        ), @r#"
         × the `/// conda-script` block has no closing `/// end-conda-script` marker
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:1:1]
        1 │ // /// conda-script
          · ─────────┬─────────
          ·          ╰── the block opens here
        2 │ // channels = ["conda-forge"]
        3 │ int main(void) {}
          · ────────┬────────
          ·         ╰── this line does not start with the block's prefix
        4 │ // /// end-conda-script
          ╰────
         help: every line of the block must start with its comment prefix "// "
        "#);
    }

    #[test]
    fn errors_on_multiple_blocks() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\nprint()\n# /// conda-script\n# channels = [\"bioconda\"]\n# /// end-conda-script\n"
        ), @r#"
         × the file contains more than one conda-script block
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:1:1]
        1 │ # /// conda-script
          · ─────────┬────────
          ·          ╰── the first block opens here
        2 │ # channels = ["conda-forge"]
          ╰────
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:6:1]
        5 │ print()
        6 │ # /// conda-script
          · ─────────┬────────
          ·          ╰── a second block opens here
        7 │ # channels = ["bioconda"]
          ╰────
         help: a file may contain at most one conda-script block
        "#);
    }

    #[test]
    fn errors_on_a_second_opening_marker_inside_the_block() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# /// conda-script\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n"
        ), @r#"
         × the file contains more than one conda-script block
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:3:1]
        1 │ # /// conda-script
          · ─────────┬────────
          ·          ╰── the first block opens here
        2 │ # channels = ["conda-forge"]
        3 │ # /// conda-script
          · ─────────┬────────
          ·          ╰── a second block opens here
        4 │ # entrypoint = "python ${SCRIPT}"
          ╰────
         help: a file may contain at most one conda-script block
        "#);
    }

    #[test]
    fn errors_on_a_second_block_with_another_prefix() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\nprint()\n// /// conda-script\n// channels = [\"bioconda\"]\n// /// end-conda-script\n"
        ), @r#"
         × the file contains more than one conda-script block
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:1:1]
        1 │ # /// conda-script
          · ─────────┬────────
          ·          ╰── the first block opens here
        2 │ # channels = ["conda-forge"]
          ╰────
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:6:1]
        5 │ print()
        6 │ // /// conda-script
          · ─────────┬─────────
          ·          ╰── a second block opens here
        7 │ // channels = ["bioconda"]
          ╰────
         help: a file may contain at most one conda-script block
        "#);
    }

    #[test]
    fn errors_on_a_pep_723_block_next_to_a_conda_script_block() {
        insta::assert_snapshot!(parse_error(
            "# /// script\n# dependencies = []\n# ///\n\n# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n"
        ), @r#"
         × the file contains both a PEP 723 `script` block and a conda-script block
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:1:1]
        1 │ # /// script
          · ──────┬─────
          ·       ╰── the PEP 723 block opens here
        2 │ # dependencies = []
          ╰────
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:1]
        4 │
        5 │ # /// conda-script
          · ─────────┬────────
          ·          ╰── the conda-script block opens here
        6 │ # channels = ["conda-forge"]
          ╰────
         help: keep either the PEP 723 block or the conda-script block, not both
        "#);
    }

    #[test]
    fn toml_syntax_errors_point_into_the_original_file() {
        insta::assert_snapshot!(parse_error(
            "// /// conda-script\n// channels = [\"conda-forge\"\n// entrypoint = \"python ${SCRIPT}\"\n// /// end-conda-script\n"
        ), @r#"
         × expected a right bracket, found an identifier
         ╰─▶ expected a right bracket, found an identifier
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:3:4]
        2 │ // channels = ["conda-forge"
        3 │ // entrypoint = "python ${SCRIPT}"
          ·    ─────────────
        4 │ // /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn errors_on_an_unknown_top_level_key() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# platforms = [\"linux-64\"]\n# /// end-conda-script\n"
        ), @r#"
         × Unexpected keys, expected only 'channels', 'entrypoint', 'dependencies', 'tool'
         ╰─▶ Unexpected keys, expected only 'channels', 'entrypoint', 'dependencies', 'tool'
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:4:3]
        3 │ # entrypoint = "python ${SCRIPT}"
        4 │ # platforms = ["linux-64"]
          ·   ────┬────
          ·       ╰── 'platforms' was not expected here
        5 │ # /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn errors_on_missing_required_keys() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# [dependencies]\n# python = \"*\"\n# /// end-conda-script\n"
        ), @r#"
         × missing field 'channels' in table
         ╰─▶ missing field 'channels' in table
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:2:3]
        1 │     # /// conda-script
        2 │ ╭─▶ # [dependencies]
        3 │ ╰─▶ # python = "*"
        4 │     # /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn errors_on_empty_channels() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = []\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n"
        ), @r#"
         × `channels` must list at least one channel
         ╰─▶ `channels` must list at least one channel
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:2:14]
        1 │ # /// conda-script
        2 │ # channels = []
          ·              ──
        3 │ # entrypoint = "python ${SCRIPT}"
          ╰────
        "#);
    }

    #[test]
    fn errors_on_a_channel_that_is_empty() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\", \"\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n"
        ), @r#"
         × a channel must not be empty
         ╰─▶ a channel must not be empty
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:2:30]
        1 │ # /// conda-script
        2 │ # channels = ["conda-forge", ""]
          ·                              ─
        3 │ # entrypoint = "python ${SCRIPT}"
          ╰────
        "#);
    }

    #[test]
    fn errors_on_a_dependency_name_that_is_empty() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# \"\" = \"*\"\n# /// end-conda-script\n"
        ), @r#"
         × a dependency name must not be empty
         ╰─▶ a dependency name must not be empty
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:3]
        4 │ # [dependencies]
        5 │ # "" = "*"
          ·   ─
        6 │ # /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn errors_on_a_dependency_name_with_invalid_characters() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# \"py thon\" = \"*\"\n# /// end-conda-script\n"
        ), @r#"
         × `py thon` is not a package name
         ╰─▶ `py thon` is not a package name
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:4]
        4 │ # [dependencies]
        5 │ # "py thon" = "*"
          ·    ───────
        6 │ # /// end-conda-script
          ╰────
         help: package names consist of letters, digits, `-`, `_` and `.`
        "#);
    }

    #[test]
    fn errors_on_an_unknown_dependency_key() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# python = { version = \"*\", build-string = \"*cuda*\" }\n# /// end-conda-script\n"
        ), @r#"
         × Unexpected keys, expected only 'version', 'build', 'build-number', 'channel', 'subdir', 'extras', 'flags', 'md5', 'sha256', 'url', 'when'
         ╰─▶ Unexpected keys, expected only 'version', 'build', 'build-number', 'channel', 'subdir', 'extras', 'flags', 'md5', 'sha256', 'url', 'when'
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:29]
        4 │ # [dependencies]
        5 │ # python = { version = "*", build-string = "*cuda*" }
          ·                             ──────┬─────
          ·                                   ╰── 'build-string' was not expected here
        6 │ # /// end-conda-script
          ╰────
         help: Did you mean 'build'?
        "#);
    }

    #[test]
    fn errors_on_a_git_dependency_outside_tool_pixi() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# demo = { git = \"https://github.com/org/repo.git\" }\n# /// end-conda-script\n"
        ), @r#"
         × Unexpected keys, expected only 'version', 'build', 'build-number', 'channel', 'subdir', 'extras', 'flags', 'md5', 'sha256', 'url', 'when'
         ╰─▶ Unexpected keys, expected only 'version', 'build', 'build-number', 'channel', 'subdir', 'extras', 'flags', 'md5', 'sha256', 'url', 'when'
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:12]
        4 │ # [dependencies]
        5 │ # demo = { git = "https://github.com/org/repo.git" }
          ·            ─┬─
          ·             ╰── 'git' was not expected here
        6 │ # /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn errors_on_url_combined_with_version() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# zlib = { url = \"https://example.com/zlib-1.3-h123_0.conda\", version = \"1.3.*\" }\n# /// end-conda-script\n"
        ), @r#"
         × `url` cannot be combined with `version`
         ╰─▶ `url` cannot be combined with `version`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:63]
        4 │ # [dependencies]
        5 │ # zlib = { url = "https://example.com/zlib-1.3-h123_0.conda", version = "1.3.*" }
          ·                                                               ───────
        6 │ # /// end-conda-script
          ╰────
         help: the URL already determines the artifact; only `md5` and `sha256` may accompany it
        "#);
    }

    #[test]
    fn errors_on_url_combined_with_when() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# zlib = { url = \"https://example.com/zlib-1.3-h123_0.conda\", when = \"__unix\" }\n# /// end-conda-script\n"
        ), @r#"
         × pixi cannot combine `url` with `when` yet
         ╰─▶ pixi cannot combine `url` with `when` yet
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:63]
        4 │ # [dependencies]
        5 │ # zlib = { url = "https://example.com/zlib-1.3-h123_0.conda", when = "__unix" }
          ·                                                               ────
        6 │ # /// end-conda-script
          ╰────
         help: the specification allows it, but pixi does not support conditions on URL specs so far
        "#);
    }

    #[test]
    fn errors_on_a_url_that_is_not_a_conda_archive() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# demo = { url = \"https://example.com/demo.tar.gz\" }\n# /// end-conda-script\n"
        ), @r#"
         × `url` must point at a conda package archive
         ╰─▶ `url` must point at a conda package archive
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:10]
        4 │ # [dependencies]
        5 │ # demo = { url = "https://example.com/demo.tar.gz" }
          ·          ───────────────────────────────────────────
        6 │ # /// end-conda-script
          ╰────
         help: source dependencies are not part of the conda-script specification; declare them under `[tool.pixi.dependencies]`
        "#);
    }

    #[test]
    fn errors_on_an_unknown_entrypoint_platform() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = { linux = \"run ${SCRIPT}\", commodore = \"load ${SCRIPT}\" }\n# /// end-conda-script\n"
        ), @r#"
         × `commodore` is not a platform
         ╰─▶ `commodore` is not a platform
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:3:43]
        2 │ # channels = ["conda-forge"]
        3 │ # entrypoint = { linux = "run ${SCRIPT}", commodore = "load ${SCRIPT}" }
          ·                                           ─────────
        4 │ # /// end-conda-script
          ╰────
         help: the entrypoint table takes platforms like `linux-64` and `win-64`, or the families `unix`, `linux`, `osx` and `win`
        "#);
    }

    /// `format_diagnostic` turns every backslash into a forward slash, so the
    /// escaped `\u{1b}` of the message reaches the snapshot as `/u{1b}`.
    #[test]
    fn an_entrypoint_key_with_control_characters_is_escaped_in_the_error() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = { \"\\u001B[31m\" = \"run ${SCRIPT}\" }\n# /// end-conda-script\n"
        ), @r#"
         × `/u{1b}[31m` is not a platform
         ╰─▶ `/u{1b}[31m` is not a platform
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:3:19]
        2 │ # channels = ["conda-forge"]
        3 │ # entrypoint = { "/u001B[31m" = "run ${SCRIPT}" }
          ·                   ──────────
        4 │ # /// end-conda-script
          ╰────
         help: the entrypoint table takes platforms like `linux-64` and `win-64`, or the families `unix`, `linux`, `osx` and `win`
        "#);
    }

    #[test]
    fn errors_on_an_empty_entrypoint_table() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = {}\n# /// end-conda-script\n"
        ), @r#"
         × the entrypoint table must contain at least one platform key
         ╰─▶ the entrypoint table must contain at least one platform key
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:3:16]
        2 │ # channels = ["conda-forge"]
        3 │ # entrypoint = {}
          ·                ──
        4 │ # /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn errors_on_an_unsupported_tool_pixi_key() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.tasks]\n# test = \"pytest\"\n# /// end-conda-script\n"
        ), @r#"
         × conda-script blocks do not support `tool.pixi.tasks`
         ╰─▶ conda-script blocks do not support `tool.pixi.tasks`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:4:14]
        3 │ # entrypoint = "python ${SCRIPT}"
        4 │ # [tool.pixi.tasks]
          ·              ─────
        5 │ # test = "pytest"
          ╰────
         help: a script represents one implicit default environment
        "#);
    }

    #[test]
    fn errors_on_package_names_that_collide_after_normalization() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [dependencies]\n# Python = \"*\"\n# python = \"3.13.*\"\n# /// end-conda-script\n"
        ), @r#"
         × duplicate key: `python`
         ╰─▶ duplicate key: `python`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:6:3]
        4 │ # [dependencies]
        5 │ # Python = "*"
          ·   ───┬──
          ·      ╰── first defined here
        6 │ # python = "3.13.*"
          ·   ───┬──
          ·      ╰── duplicate defined here
        7 │ # /// end-conda-script
          ╰────
        "#);
    }

    #[test]
    fn edits_write_back_through_the_comment_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.c");
        fs_err::write(
            &path,
            "#include <zlib.h>\n// /// conda-script\n// channels = [\"conda-forge\"]\n// entrypoint = \"run ${SCRIPT}\"\n//\n// [dependencies]\n// gcc = \"*\"\n// /// end-conda-script\nint main(void) { return 0; }\n",
        )
        .unwrap();

        let manifest = CondaScriptManifest::from_path(&path).unwrap().unwrap();
        let mut metadata = manifest.metadata_document().unwrap();
        metadata["dependencies"]["zlib"] = toml_edit::value("1.3.*");
        manifest.write_metadata(&metadata).unwrap();

        insta::assert_snapshot!(fs_err::read_to_string(&path).unwrap(), @r#"
        #include <zlib.h>
        // /// conda-script
        // channels = ["conda-forge"]
        // entrypoint = "run ${SCRIPT}"
        //
        // [dependencies]
        // gcc = "*"
        // zlib = "1.3.*"
        // /// end-conda-script
        int main(void) { return 0; }
        "#);
    }

    #[test]
    fn editing_preserves_crlf_line_endings_and_the_code_around_the_block() {
        let contents = "print()\r\n# /// conda-script\r\n# channels = [\"conda-forge\"]\r\n# entrypoint = \"python ${SCRIPT}\"\r\n# /// end-conda-script\r\nprint('after')\r\n";
        let manifest = parse(contents).unwrap().unwrap();
        let metadata = manifest.metadata_document().unwrap();
        let rendered = manifest.render_metadata(&metadata);
        assert_eq!(rendered, contents);
    }
}
