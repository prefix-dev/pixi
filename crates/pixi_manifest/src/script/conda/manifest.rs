use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use indexmap::{IndexMap, IndexSet};
use miette::NamedSource;
use toml_edit::{DocumentMut, Item};
use toml_span::{Span, Spanned};

use super::{
    CondaScriptError, CondaScriptMetadata, CondaScriptTemplate,
    envelope::{self, CLOSING_MARKER, OPENING_MARKER},
    error::{EnvelopeError, EnvelopeErrorKind, MetadataError},
    metadata::ParsedBlock,
};
use crate::{
    EnvironmentName, FeatureName, InvalidRequiresPixiError, PixiPlatform, PixiVersionMismatchError,
    TomlError, Warning, WorkspaceManifest,
    discovery::RequiresPixiCheck,
    script::{
        ScriptWorkspaceConfig,
        block::{BlockSourceMap, LineEnding, extract_script_header, serialize_block},
    },
    toml::{ExternalWorkspaceProperties, PackageDefaults, TomlEnvironmentList, TomlFeature},
    utils::PixiSpanned,
};

/// The feature that carries `tool.pixi.dependencies` in the workspace.
const TOOL_PIXI_FEATURE: &str = "tool-pixi";

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
    source: Arc<str>,
    source_map: BlockSourceMap,
}

impl CondaScriptManifest {
    /// Add a `conda-script` block to a new or existing file.
    ///
    /// The block is generated from the language template; `channels`
    /// overrides the template's channels when non-empty. A freshly created
    /// file also gets the template's starter program.
    pub fn initialize(
        path: impl AsRef<Path>,
        template: &CondaScriptTemplate,
        channels: &[String],
    ) -> Result<Self, CondaScriptError> {
        let path = std::path::absolute(path)?;
        let contents = match fs_err::read(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        if Self::from_source(&path, &contents)?.is_some() {
            return Err(CondaScriptError::AlreadyInitialized { path });
        }

        let mut metadata = toml_edit::DocumentMut::new();
        let mut channel_array = toml_edit::Array::new();
        if channels.is_empty() {
            channel_array.extend(template.channels.iter().copied());
        } else {
            channel_array.extend(channels.iter().map(String::as_str));
        }
        metadata.insert(
            "channels",
            toml_edit::Item::Value(toml_edit::Value::Array(channel_array)),
        );
        metadata.insert("entrypoint", toml_edit::value(template.entrypoint));
        let mut dependencies = toml_edit::Table::new();
        for dependency in template.dependencies {
            dependencies.insert(dependency, toml_edit::value("*"));
        }
        metadata.insert("dependencies", toml_edit::Item::Table(dependencies));

        let line_ending = LineEnding::detect(&contents);
        let (bom, shebang, body) = extract_script_header(&contents)?;
        let body = if contents.is_empty() {
            template.body
        } else {
            body
        };

        let mut output = String::new();
        output.push_str(bom);
        if let Some(shebang) = shebang {
            output.push_str(shebang);
            output.push_str(line_ending.as_str());
            output.push_str(template.prefix.trim_end());
            output.push_str(line_ending.as_str());
        }
        output.push_str(&serialize_block(
            &metadata.to_string(),
            template.prefix,
            &format!("{}{OPENING_MARKER}", template.prefix),
            &format!("{}{CLOSING_MARKER}", template.prefix),
            line_ending.as_str(),
        ));
        if !body.is_empty() {
            output.push_str(line_ending.as_str());
            output.push_str(body);
        }

        // Parsing before writing keeps a file that would not read back, say
        // one that already carries a PEP 723 block, untouched on disk.
        let manifest = Self::from_source(&path, output.as_bytes())?
            .expect("a block serialized by the script initializer must be parseable");

        fs_err::create_dir_all(
            path.parent()
                .expect("an absolute script path always has a parent"),
        )?;
        fs_err::write(&path, output)?;

        Ok(manifest)
    }

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

        let parsed = match ParsedBlock::from_toml_str(&block.metadata) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(Box::new(MetadataError {
                    error,
                    source: NamedSource::new(source_name, source),
                    source_map: block.source_map,
                })
                .into());
            }
        };
        match parsed.requires_pixi {
            RequiresPixiCheck::Satisfied => {}
            RequiresPixiCheck::Mismatch {
                requires_pixi,
                span,
            } => {
                return Err(Box::new(PixiVersionMismatchError {
                    requires_pixi,
                    source_code: NamedSource::new(source_name, source),
                    span: block.source_map.span(span.offset(), span.len(), 0),
                })
                .into());
            }
            RequiresPixiCheck::Invalid { span, parse_error } => {
                return Err(Box::new(InvalidRequiresPixiError {
                    source_code: NamedSource::new(source_name, source),
                    span: block.source_map.span(span.offset(), span.len(), 0),
                    parse_error,
                })
                .into());
            }
        }
        let metadata = parsed.metadata;

        Ok(Some(Self {
            path,
            metadata,
            toml: block.metadata,
            prefix: block.prefix,
            prelude: block.prelude,
            postlude: block.postlude,
            line_ending: LineEnding::detect(contents),
            source,
            source_map: block.source_map,
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

    /// Whether the block pins channels and platforms itself.
    ///
    /// Channels are always explicit, the block requires them. Platforms are
    /// explicit when `tool.pixi.workspace.platforms` is declared; otherwise
    /// the script resolves for the machine it runs on.
    pub fn workspace_config(&self) -> Result<ScriptWorkspaceConfig, CondaScriptError> {
        let metadata = self.metadata_document()?;
        let platforms_explicit = metadata
            .get("tool")
            .and_then(Item::as_table_like)
            .and_then(|tool| tool.get("pixi"))
            .and_then(Item::as_table_like)
            .and_then(|pixi| pixi.get("workspace"))
            .and_then(Item::as_table_like)
            .is_some_and(|workspace| workspace.contains_key("platforms"));
        Ok(ScriptWorkspaceConfig {
            channels_explicit: true,
            platforms_explicit,
        })
    }

    /// The workspace the script resolves in.
    ///
    /// `tool.pixi` is the manifest, with the block's `[dependencies]` as its
    /// dependency table, where the `--script` editors expect them.
    /// `tool.pixi.dependencies` move into a feature of the default
    /// environment, so a package named in both merges the way pixi merges
    /// features: both specs reach the solver. The workspace is named after
    /// the file.
    pub fn into_workspace_manifest(
        &self,
        implicit_platforms: Option<IndexSet<PixiPlatform>>,
    ) -> Result<(WorkspaceManifest, Vec<Warning>), CondaScriptError> {
        let ParsedBlock {
            dependencies,
            mut manifest,
            ..
        } = ParsedBlock::from_toml_str(&self.toml).map_err(|error| self.metadata_error(error))?;

        let workspace = &mut manifest
            .workspace
            .as_mut()
            .expect("the block parser always sets a workspace")
            .value;
        workspace.name = Some(self.name().to_owned());
        if let Some(platforms) = implicit_platforms {
            workspace.platforms = Spanned {
                value: platforms,
                span: Span::default(),
            };
        }

        let pixi_dependencies = std::mem::replace(&mut manifest.dependencies, dependencies);
        if let Some(pixi_dependencies) = pixi_dependencies {
            let feature = TomlFeature {
                dependencies: Some(pixi_dependencies),
                ..TomlFeature::default()
            };
            manifest.feature = Some(PixiSpanned {
                span: None,
                value: IndexMap::from([(
                    PixiSpanned {
                        span: None,
                        value: FeatureName::from(TOOL_PIXI_FEATURE.to_owned()),
                    },
                    feature,
                )]),
            });
            manifest.environments = Some(PixiSpanned {
                span: None,
                value: IndexMap::from([(
                    EnvironmentName::Default,
                    TomlEnvironmentList::Seq(Spanned {
                        value: vec![Spanned {
                            value: TOOL_PIXI_FEATURE.to_owned(),
                            span: Span::default(),
                        }],
                        span: Span::default(),
                    }),
                )]),
            });
        }

        let root = self
            .path
            .parent()
            .expect("an absolute script path always has a parent");
        let (workspace, package, warnings) = manifest
            .into_workspace_manifest(
                ExternalWorkspaceProperties::default(),
                PackageDefaults::default(),
                root,
            )
            .map_err(|error| self.metadata_error(error))?;
        debug_assert!(package.is_none(), "a script never defines a package");
        Ok((workspace, warnings))
    }

    /// The workspace name: the file stem, or `script` for a file without one.
    fn name(&self) -> &str {
        self.path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("script")
    }

    fn metadata_error(&self, error: TomlError) -> CondaScriptError {
        Box::new(MetadataError {
            error,
            source: NamedSource::new(self.path.to_string_lossy(), Arc::clone(&self.source)),
            source_map: self.source_map.clone(),
        })
        .into()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_pypi_spec::PypiPackageName;
    use pixi_spec::PixiSpec;
    use pixi_test_utils::format_diagnostic;
    use rattler_conda_types::{PackageName, Platform};

    use super::super::Entrypoint;
    use super::*;
    use crate::{KnownPreviewFlag, PixiPlatform, SpecType};

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
# [tool.pixi.workspace]
# preview = ["pixi-build"]
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

        let (workspace, warnings) = manifest.into_workspace_manifest(None).unwrap();
        assert!(warnings.is_empty());
        let target = workspace.default_feature().targets.default();
        let simple_app = workspace
            .feature(&FeatureName::from(TOOL_PIXI_FEATURE.to_owned()))
            .unwrap()
            .targets
            .default()
            .run_dependencies()
            .unwrap()
            .get(&PackageName::new_unchecked("simple-app"))
            .unwrap();
        assert!(simple_app.iter().any(PixiSpec::is_source));
        assert!(
            target
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .contains_key(&PypiPackageName::from_str("requests").unwrap())
        );
    }

    #[test]
    fn tool_pixi_tables_reach_the_workspace() {
        let manifest = parse(
            r#"# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "python ${SCRIPT}"
#
# [tool.pixi.workspace]
# platforms = ["linux-64", "win-64"]
# exclude-newer = "2025-01-01"
#
# [tool.pixi.activation.env]
# GREETING = "hello"
#
# [tool.pixi.constraints]
# openssl = ">=3"
#
# [tool.pixi.exclude-newer]
# zlib = "0d"
#
# [tool.pixi.target.win-64.dependencies]
# vc = "*"
# /// end-conda-script
"#,
        )
        .unwrap()
        .unwrap();

        let (workspace, warnings) = manifest.into_workspace_manifest(None).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            workspace
                .workspace
                .platforms
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["linux-64", "win-64"]
        );
        assert!(workspace.workspace.exclude_newer.is_some());
        assert!(
            workspace
                .workspace
                .exclude_newer_package_overrides
                .contains_key(&PackageName::new_unchecked("zlib"))
        );
        let feature = workspace.default_feature();
        assert_eq!(
            feature
                .targets
                .default()
                .activation
                .as_ref()
                .and_then(|activation| activation.env.as_ref())
                .and_then(|env| env.get("GREETING"))
                .map(String::as_str),
            Some("hello")
        );
        assert!(feature.targets.default().constraints.is_some());
        let vc = PackageName::new_unchecked("vc");
        let has_vc = |platform: Platform| {
            feature
                .run_dependencies(Some(&PixiPlatform::from(platform)))
                .is_some_and(|dependencies| dependencies.contains_key(&vc))
        };
        assert!(has_vc(Platform::Win64));
        assert!(!has_vc(Platform::Linux64));
    }

    #[test]
    fn workspace_config_reports_declared_platforms() {
        let implicit = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();
        let config = implicit.workspace_config().unwrap();
        assert!(config.channels_explicit);
        assert!(!config.platforms_explicit);

        let explicit = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.workspace]\n# platforms = [\"linux-64\"]\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();
        assert!(explicit.workspace_config().unwrap().platforms_explicit);
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
    fn tool_pixi_dependencies_form_a_feature_of_the_default_environment() {
        let manifest = parse(
            r#"// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "python ${SCRIPT}"
//
// [dependencies]
// python = "3.13.*"
// simple-app = "0.1.*"
// gcc = { version = "*", when = "__unix" }
//
// [tool.pixi.workspace]
// preview = ["pixi-build"]
//
// [tool.pixi.dependencies]
// simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git" }
// /// end-conda-script
"#,
        )
        .unwrap()
        .unwrap();

        let (workspace, warnings) = manifest.into_workspace_manifest(None).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(workspace.workspace.name.as_deref(), Some("example"));
        assert_eq!(
            workspace
                .workspace
                .channels
                .iter()
                .map(|channel| channel.channel.to_string())
                .collect::<Vec<_>>(),
            ["conda-forge"]
        );
        assert!(workspace.workspace.platforms.is_empty());
        assert!(
            workspace
                .workspace
                .preview
                .is_enabled(KnownPreviewFlag::PixiBuild)
        );

        let tool_pixi_feature = FeatureName::from(TOOL_PIXI_FEATURE.to_owned());
        assert_eq!(workspace.environments.iter().count(), 1);
        assert!(
            workspace
                .default_environment()
                .features
                .contains(&tool_pixi_feature)
        );

        let simple_app = PackageName::new_unchecked("simple-app");
        let block = workspace.default_feature().targets.default();
        assert!(block.has_dependency(&simple_app, SpecType::Run, None));
        assert!(block.has_dependency(&PackageName::new_unchecked("gcc"), SpecType::Run, None));
        let pixi = workspace
            .feature(&tool_pixi_feature)
            .unwrap()
            .targets
            .default();
        assert!(
            pixi.run_dependencies()
                .unwrap()
                .get(&simple_app)
                .unwrap()
                .iter()
                .any(PixiSpec::is_source)
        );
    }

    #[test]
    fn a_source_dependency_needs_the_declared_preview() {
        let manifest = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.dependencies]\n# simple-app = { git = \"https://github.com/prefix-dev/pixi-build-testsuite.git\" }\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();

        insta::assert_snapshot!(
            format_diagnostic(&manifest.into_workspace_manifest(None).unwrap_err()),
            @r#"
         × conda source dependencies are not allowed without enabling the 'pixi-build' preview flag
         ╰─▶ conda source dependencies are not allowed without enabling the 'pixi-build' preview flag
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:16]
        4 │ # [tool.pixi.dependencies]
        5 │ # simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git" }
          ·                ─────────────────────────────────┬────────────────────────────────
          ·                                                 ╰── source dependency specified here
        6 │ # /// end-conda-script
          ╰────
         help: Run `pixi workspace preview add pixi-build` to enable the preview flag
        "#
        );
    }

    #[test]
    fn a_block_without_tool_pixi_dependencies_has_only_the_default_feature() {
        let manifest = parse(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# /// end-conda-script\n",
        )
        .unwrap()
        .unwrap();

        let (workspace, _) = manifest.into_workspace_manifest(None).unwrap();
        assert_eq!(workspace.all_features().count(), 1);
        assert_eq!(workspace.environments.iter().count(), 1);
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
         × scripts do not support `tool.pixi.tasks`
         ╰─▶ scripts do not support `tool.pixi.tasks`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:4:14]
        3 │ # entrypoint = "python ${SCRIPT}"
        4 │ # [tool.pixi.tasks]
          ·              ─────
        5 │ # test = "pytest"
          ╰────
         help: a script represents one implicit default environment; `tool.pixi` accepts `activation`, `constraints`, `dependencies`, `exclude-newer`, `pypi-dependencies`, `pypi-exclude-newer`, `target`,
               `workspace`
        "#);
    }

    #[test]
    fn errors_on_a_feature_table() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.feature.test.dependencies]\n# pytest = \"*\"\n# [tool.pixi.environments]\n# test = [\"test\"]\n# /// end-conda-script\n"
        ), @r#"
         × scripts do not support `tool.pixi.feature` and `tool.pixi.environments`
         ╰─▶ scripts do not support `tool.pixi.feature` and `tool.pixi.environments`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:4:14]
        3 │ # entrypoint = "python ${SCRIPT}"
        4 │ # [tool.pixi.feature.test.dependencies]
          ·              ───────
        5 │ # pytest = "*"
        6 │ # [tool.pixi.environments]
          ·              ────────────
        7 │ # test = ["test"]
          ╰────
         help: a script represents one implicit default environment; `tool.pixi` accepts `activation`, `constraints`, `dependencies`, `exclude-newer`, `pypi-dependencies`, `pypi-exclude-newer`, `target`,
               `workspace`
        "#);
    }

    #[test]
    fn errors_on_workspace_channels_in_tool_pixi() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.workspace]\n# channels = [\"bioconda\"]\n# /// end-conda-script\n"
        ), @r#"
         × scripts do not support `tool.pixi.workspace.channels`
         ╰─▶ scripts do not support `tool.pixi.workspace.channels`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:3]
        4 │ # [tool.pixi.workspace]
        5 │ # channels = ["bioconda"]
          ·   ────────
        6 │ # /// end-conda-script
          ╰────
         help: a conda-script block declares its channels at the top level
        "#);
    }

    #[test]
    fn errors_on_system_requirements() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.system-requirements]\n# cuda = \"12\"\n# /// end-conda-script\n"
        ), @r#"
         × scripts do not support `tool.pixi.system-requirements`
         ╰─▶ scripts do not support `tool.pixi.system-requirements`
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:4:14]
        3 │ # entrypoint = "python ${SCRIPT}"
        4 │ # [tool.pixi.system-requirements]
          ·              ───────────────────
        5 │ # cuda = "12"
          ╰────
         help: a script without `platforms` resolves for the virtual packages of the machine it runs on; declare `platforms` with their virtual packages to pin a target instead
        "#);
    }

    #[test]
    fn typed_errors_in_tool_pixi_point_into_the_file() {
        insta::assert_snapshot!(parse_error(
            "# /// conda-script\n# channels = [\"conda-forge\"]\n# entrypoint = \"python ${SCRIPT}\"\n# [tool.pixi.dependencies]\n# bad-package = \"!invalid!\"\n# /// end-conda-script\n"
        ), @r#"
         × invalid operator '!'
         ╰─▶ invalid operator '!'
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:5:18]
        4 │ # [tool.pixi.dependencies]
        5 │ # bad-package = "!invalid!"
          ·                  ─────────
        6 │ # /// end-conda-script
          ╰────
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
