//! Parsing of inline script metadata embedded in script files.
//!
//! A script can describe the conda environment it needs by embedding a
//! [PEP 723](https://peps.python.org/pep-0723/) style `script` metadata block,
//! using the same format as
//! [conda-exec](https://conda-incubator.github.io/conda-exec/):
//!
//! ```python
//! #!/usr/bin/env python
//! # /// script
//! # requires-python = ">=3.12"
//! #
//! # [tool.conda]
//! # channels = ["conda-forge"]
//! # dependencies = ["requests"]
//! #
//! # [tool.pixi]
//! # entrypoint = "python"
//! # ///
//! ```
//!
//! The block opens with a `/// script` line and closes with a `///` line, each
//! carried by the file's line-comment marker (`#`, `//`, or `--`; block
//! comments are not supported). Every line in between must carry the same
//! marker; the marker and at most one following space are stripped, and the
//! remaining lines form the TOML document described by [`ScriptMetadata`].
//! Following PEP 723, the block ends at the *last* `///` line of the comment
//! block, and only the first block in a file is read.
//!
//! Conda dependencies and channels live in the `[tool.conda]` table shared
//! with conda-exec, while pixi-specific configuration such as the entrypoint
//! and platform-specific overrides live in `[tool.pixi]`.

pub mod lock;

use std::str::FromStr;

use indexmap::IndexMap;
use pixi_manifest::{PixiPlatform, TargetSelector};
use rattler_conda_types::{
    MatchSpec, NamedChannelOrUrl, NamelessMatchSpec, ParseStrictness, Platform,
};
use serde::Deserialize;
use thiserror::Error;

/// Opens a metadata block (after the comment marker).
const BLOCK_START: &str = "/// script";
/// Closes a metadata block (after the comment marker).
const BLOCK_END: &str = "///";

/// The line-comment markers that can introduce a metadata block.
const COMMENT_MARKERS: [&str; 3] = ["#", "//", "--"];

/// Errors that can occur while parsing a `script` metadata block.
#[derive(Debug, Error, miette::Diagnostic)]
pub enum ScriptMetadataError {
    #[error("the script metadata block is never closed, expected a `{marker} {BLOCK_END}` line")]
    UnclosedBlock { marker: String },

    #[error("failed to parse the TOML in the script metadata block")]
    Toml(#[from] toml_edit::de::Error),

    #[error(
        "PyPI dependencies in script metadata are not supported yet; declare conda packages under `[tool.conda]` instead"
    )]
    PyPiDependenciesNotSupported,

    #[error("`{spec}` is not a valid `requires-python` specifier")]
    InvalidRequiresPython {
        spec: String,
        #[source]
        source: rattler_conda_types::ParseMatchSpecError,
    },

    #[error(
        "`{selector}` is not a valid platform selector, expected `unix`, `linux`, `osx`, `win`, or a conda subdir such as `linux-64`"
    )]
    InvalidTargetSelector { selector: String },

    #[error("`{spec}` is not a valid conda match spec")]
    InvalidMatchSpec {
        spec: String,
        #[source]
        source: rattler_conda_types::ParseMatchSpecError,
    },
}

/// The raw TOML document embedded in a metadata block, following PEP 723:
/// `requires-python` and `dependencies` at the top level and tool-specific
/// configuration under `[tool.<name>]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct TomlScriptMetadata {
    /// PEP 723 Python version constraint, translated into a conda `python`
    /// dependency.
    #[serde(default)]
    requires_python: Option<String>,
    /// PEP 723 PyPI dependencies. Not supported (yet); rejected when
    /// non-empty.
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    tool: TomlTool,
}

/// The `[tool]` table. Tables of tools other than conda and pixi are ignored.
#[derive(Debug, Default, Deserialize)]
struct TomlTool {
    #[serde(default)]
    conda: TomlConda,
    #[serde(default)]
    pixi: TomlPixi,
}

/// The `[tool.conda]` table shared with conda-exec: where the packages come
/// from and which conda packages the script needs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlConda {
    #[serde(default)]
    channels: Option<Vec<NamedChannelOrUrl>>,
    /// Conda match specs, e.g. `["python 3.12.*", "samtools>=1.19"]`.
    #[serde(default)]
    dependencies: Vec<String>,
}

/// The `[tool.pixi]` table: pixi-specific execution configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlPixi {
    /// The command that runs the script.
    #[serde(default)]
    entrypoint: Option<String>,
    /// Platform-specific overrides: `[tool.pixi.target.<selector>]`.
    #[serde(default)]
    target: IndexMap<String, TomlPixiTarget>,
}

/// A `[tool.pixi.target.<selector>]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlPixiTarget {
    /// Additional conda match specs for the matching platforms.
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    entrypoint: Option<String>,
}

/// The dependencies and execution configuration parsed from a script's
/// metadata block.
#[derive(Debug, Clone)]
pub struct ScriptMetadata {
    /// The TOML document the block contained, verbatim. Hashed to detect
    /// whether a lock file is still up-to-date.
    document: String,
    /// Dependencies that apply to every platform, including the `python` spec
    /// derived from `requires-python`.
    dependencies: Vec<MatchSpec>,
    /// Whether the script declared a Python requirement (`requires-python` or
    /// an explicit `python` dependency).
    requires_python: bool,
    /// Additional dependencies for the platforms matching the selector.
    target_dependencies: Vec<(TargetSelector, Vec<MatchSpec>)>,
    /// The channels to install the dependencies from, when specified.
    channels: Option<Vec<NamedChannelOrUrl>>,
    /// The default command used to run the script.
    entrypoint: Option<String>,
    /// Platform-specific overrides for the entrypoint.
    target_entrypoints: Vec<(TargetSelector, String)>,
}

impl ScriptMetadata {
    /// Parses the metadata block from script source. Returns `Ok(None)` when
    /// the source does not contain a `script` block.
    pub fn from_source(source: &str) -> Result<Option<Self>, ScriptMetadataError> {
        let Some(document) = extract_metadata_block(source)? else {
            return Ok(None);
        };
        let raw: TomlScriptMetadata = toml_edit::de::from_str(&document)?;
        Self::from_raw(raw, document).map(Some)
    }

    /// Validates the raw TOML document and converts it into typed form.
    fn from_raw(raw: TomlScriptMetadata, document: String) -> Result<Self, ScriptMetadataError> {
        if !raw.dependencies.is_empty() {
            return Err(ScriptMetadataError::PyPiDependenciesNotSupported);
        }

        let mut dependencies = parse_dependencies(&raw.tool.conda.dependencies)?;
        if let Some(requires_python) = &raw.requires_python {
            dependencies.push(parse_requires_python(requires_python)?);
        }
        let requires_python = raw.requires_python.is_some()
            || dependencies.iter().any(|spec| {
                spec.name
                    .as_exact()
                    .is_some_and(|name| name.as_normalized() == "python")
            });

        let mut target_dependencies = Vec::new();
        let mut target_entrypoints = Vec::new();
        for (selector, target) in raw.tool.pixi.target {
            let selector = parse_target_selector(&selector)?;
            if !target.dependencies.is_empty() {
                target_dependencies
                    .push((selector.clone(), parse_dependencies(&target.dependencies)?));
            }
            if let Some(entrypoint) = target.entrypoint {
                target_entrypoints.push((selector, entrypoint));
            }
        }

        Ok(Self {
            document,
            dependencies,
            requires_python,
            target_dependencies,
            // An explicitly empty channel list behaves like an absent one so
            // that the caller's default channels still apply.
            channels: raw
                .tool
                .conda
                .channels
                .filter(|channels| !channels.is_empty()),
            entrypoint: raw.tool.pixi.entrypoint,
            target_entrypoints,
        })
    }

    /// The TOML document the metadata block contained, verbatim.
    pub fn document(&self) -> &str {
        &self.document
    }

    /// The dependencies that apply to `platform`.
    pub fn dependencies(&self, platform: Platform) -> Vec<MatchSpec> {
        let platform = PixiPlatform::from(platform);
        let mut specs = self.dependencies.clone();
        for (selector, target_specs) in &self.target_dependencies {
            if selector.matches(&platform) {
                specs.extend(target_specs.iter().cloned());
            }
        }
        specs
    }

    /// Whether the script declared a Python requirement, either through
    /// `requires-python` or an explicit `python` dependency.
    pub fn requires_python(&self) -> bool {
        self.requires_python
    }

    /// The channels to install the dependencies from, when the block defines
    /// any.
    pub fn channels(&self) -> Option<&[NamedChannelOrUrl]> {
        self.channels.as_deref()
    }

    /// The command that runs the script on `platform`: the entrypoint of the
    /// first matching `[tool.pixi.target.<selector>]` table, falling back to
    /// the default `entrypoint`.
    pub fn entrypoint(&self, platform: Platform) -> Option<&str> {
        let platform = PixiPlatform::from(platform);
        self.target_entrypoints
            .iter()
            .find(|(selector, _)| selector.matches(&platform))
            .map(|(_, entrypoint)| entrypoint.as_str())
            .or(self.entrypoint.as_deref())
    }
}

/// Converts the entries of a dependencies list into match specs.
fn parse_dependencies(specs: &[String]) -> Result<Vec<MatchSpec>, ScriptMetadataError> {
    specs
        .iter()
        .map(|spec| {
            MatchSpec::from_str(spec, ParseStrictness::Lenient).map_err(|source| {
                ScriptMetadataError::InvalidMatchSpec {
                    spec: spec.clone(),
                    source,
                }
            })
        })
        .collect()
}

/// Converts a PEP 723 `requires-python` specifier into a conda `python` match
/// spec.
fn parse_requires_python(specifier: &str) -> Result<MatchSpec, ScriptMetadataError> {
    let nameless =
        NamelessMatchSpec::from_str(specifier, ParseStrictness::Lenient).map_err(|source| {
            ScriptMetadataError::InvalidRequiresPython {
                spec: specifier.to_string(),
                source,
            }
        })?;
    Ok(MatchSpec::from_nameless(
        nameless,
        rattler_conda_types::PackageName::new_unchecked("python").into(),
    ))
}

/// Parses a platform selector. Scripts run outside a workspace, so only the
/// platform families (`unix`, `linux`, `osx`, `win`) and concrete conda
/// subdirs are valid; workspace-defined platform names are not.
fn parse_target_selector(selector: &str) -> Result<TargetSelector, ScriptMetadataError> {
    match TargetSelector::from_str(selector) {
        Ok(TargetSelector::Platform(_)) | Err(_) => {
            Err(ScriptMetadataError::InvalidTargetSelector {
                selector: selector.to_string(),
            })
        }
        Ok(selector) => Ok(selector),
    }
}

/// Extracts the TOML document embedded in the first `script` comment block,
/// or `None` when `source` does not contain an opening marker.
fn extract_metadata_block(source: &str) -> Result<Option<String>, ScriptMetadataError> {
    let mut lines = source.lines();

    // Find the line that opens the block and remember its comment marker; the
    // remainder of the block must use the same marker.
    let Some(marker) = lines.find_map(|line| {
        COMMENT_MARKERS
            .into_iter()
            .find(|marker| matches!(line.trim().strip_prefix(marker), Some(rest) if rest.trim() == BLOCK_START))
    }) else {
        return Ok(None);
    };

    // Collect the contiguous run of comment lines that follows. Per PEP 723
    // the *last* `///` line of the run closes the block, so that TOML content
    // may itself contain `///` lines.
    let mut content: Vec<&str> = Vec::new();
    let mut close_index = None;
    for line in lines {
        let Some(rest) = line.trim().strip_prefix(marker) else {
            // The first line without the comment marker (e.g. a blank line or
            // code) ends the comment block.
            break;
        };

        // At most one space after the marker belongs to the comment syntax
        // itself; anything beyond that is significant TOML indentation.
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        if rest.trim_end() == BLOCK_END {
            close_index = Some(content.len());
        }
        content.push(rest);
    }

    match close_index {
        Some(index) => Ok(Some(content[..index].join("\n"))),
        None => Err(ScriptMetadataError::UnclosedBlock {
            marker: marker.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_strings(specs: Vec<MatchSpec>) -> Vec<String> {
        specs.into_iter().map(|spec| spec.to_string()).collect()
    }

    #[test]
    fn parse_python_style_block() {
        let metadata = ScriptMetadata::from_source(
            r#"#!/usr/bin/env python
# Some header comment
# /// script
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["python 3.12.*", "requests"]
#
# [tool.pixi]
# entrypoint = "python"
# ///

import requests
print("Hello!")
"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            spec_strings(metadata.dependencies(Platform::Linux64)),
            ["python 3.12.*", "requests"]
        );
        assert_eq!(
            metadata.channels(),
            Some(&[NamedChannelOrUrl::Name("conda-forge".to_string())][..])
        );
        assert_eq!(metadata.entrypoint(Platform::Linux64), Some("python"));
        assert!(metadata.requires_python());
    }

    #[test]
    fn parse_conda_exec_tutorial_block() {
        // The first example from the conda-exec run-script tutorial.
        let metadata = ScriptMetadata::from_source(
            r#"# /// script
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["zlib"]
# ///

print("Hello from a conda-exec script!")
"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            spec_strings(metadata.dependencies(Platform::Linux64)),
            ["zlib"]
        );
        assert_eq!(metadata.entrypoint(Platform::Linux64), None);
        assert!(!metadata.requires_python());
    }

    #[test]
    fn requires_python_becomes_a_conda_dependency() {
        let metadata = ScriptMetadata::from_source(
            r#"# /// script
# requires-python = ">=3.12"
#
# [tool.conda]
# channels = ["conda-forge", "bioconda"]
# dependencies = ["samtools>=1.19"]
# ///
"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            spec_strings(metadata.dependencies(Platform::Linux64)),
            ["samtools >=1.19", "python >=3.12"]
        );
        assert!(metadata.requires_python());
    }

    #[test]
    fn pypi_dependencies_are_not_supported_yet() {
        let result = ScriptMetadata::from_source(
            r#"# /// script
# dependencies = ["requests", "rich"]
# ///
"#,
        );
        assert!(matches!(
            result,
            Err(ScriptMetadataError::PyPiDependenciesNotSupported)
        ));
    }

    #[test]
    fn other_tool_tables_are_ignored() {
        let metadata = ScriptMetadata::from_source(
            r#"# /// script
# [tool.conda]
# dependencies = ["zlib"]
# [tool.uv]
# exclude-newer = "2026-01-01T00:00:00Z"
# ///
"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            spec_strings(metadata.dependencies(Platform::Linux64)),
            ["zlib"]
        );
    }

    #[test]
    fn parse_slash_and_dash_comment_styles() {
        for marker in ["//", "--"] {
            let source = format!(
                "{marker} /// script\n{marker} [tool.conda]\n{marker} dependencies = [\"gcc\"]\n{marker} ///\n"
            );
            let metadata = ScriptMetadata::from_source(&source).unwrap().unwrap();
            assert_eq!(
                spec_strings(metadata.dependencies(Platform::Linux64)),
                ["gcc"]
            );
        }
    }

    #[test]
    fn last_closing_marker_wins() {
        // Per PEP 723 the last `# ///` line of the comment block closes it,
        // so embedded `///` lines in TOML strings are preserved.
        let metadata = ScriptMetadata::from_source(
            r#"# /// script
# [tool.conda]
# dependencies = ["zlib"]
# [tool.pixi]
# entrypoint = """
# ///
# """
# ///
"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(metadata.entrypoint(Platform::Linux64), Some("///\n"));
    }

    #[test]
    fn platform_specific_dependencies() {
        let metadata = ScriptMetadata::from_source(
            r#"# /// script
# [tool.conda]
# dependencies = ["python 3.12.*"]
# [tool.pixi.target.unix]
# dependencies = ["gcc"]
# [tool.pixi.target.win]
# dependencies = ["vs2022_win-64"]
# [tool.pixi.target.linux-64]
# dependencies = ["patchelf"]
# ///
"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            spec_strings(metadata.dependencies(Platform::Linux64)),
            ["python 3.12.*", "gcc", "patchelf"]
        );
        assert_eq!(
            spec_strings(metadata.dependencies(Platform::OsxArm64)),
            ["python 3.12.*", "gcc"]
        );
        assert_eq!(
            spec_strings(metadata.dependencies(Platform::Win64)),
            ["python 3.12.*", "vs2022_win-64"]
        );
    }

    #[test]
    fn platform_specific_entrypoint() {
        let metadata = ScriptMetadata::from_source(
            r#"# /// script
# [tool.pixi]
# entrypoint = "bash ${SCRIPT}"
# [tool.pixi.target.win]
# entrypoint = "cmd.exe /c ${SCRIPT}"
# ///
"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            metadata.entrypoint(Platform::Linux64),
            Some("bash ${SCRIPT}")
        );
        assert_eq!(
            metadata.entrypoint(Platform::Win64),
            Some("cmd.exe /c ${SCRIPT}")
        );
    }

    #[test]
    fn source_without_block_is_not_an_error() {
        let metadata =
            ScriptMetadata::from_source("#!/usr/bin/env python\nprint(\"hi\")\n").unwrap();
        assert!(metadata.is_none());
    }

    #[test]
    fn missing_channels_fall_back_to_caller_defaults() {
        let source = r#"# /// script
# [tool.conda]
# channels = []
# dependencies = ["python"]
# ///
"#;
        let metadata = ScriptMetadata::from_source(source).unwrap().unwrap();
        assert_eq!(metadata.channels(), None);

        let metadata = ScriptMetadata::from_source(&source.replace("# channels = []\n", ""))
            .unwrap()
            .unwrap();
        assert_eq!(metadata.channels(), None);
        assert_eq!(metadata.entrypoint(Platform::Linux64), None);
    }

    #[test]
    fn unclosed_block_is_an_error() {
        let result = ScriptMetadata::from_source(
            "# /// script\n# [tool.conda]\n# dependencies = [\"zlib\"]\n",
        );
        assert!(matches!(
            result,
            Err(ScriptMetadataError::UnclosedBlock { .. })
        ));
    }

    #[test]
    fn interrupted_block_is_an_error() {
        // A non-comment line ends the comment block; without a closing `///`
        // the block is unclosed.
        let result =
            ScriptMetadata::from_source("# /// script\n# [tool.conda]\nimport os\n# ///\n");
        assert!(matches!(
            result,
            Err(ScriptMetadataError::UnclosedBlock { .. })
        ));
    }

    #[test]
    fn invalid_target_selector_is_an_error() {
        let result = ScriptMetadata::from_source(
            "# /// script\n# [tool.pixi.target.amiga]\n# dependencies = [\"zlib\"]\n# ///\n",
        );
        assert!(matches!(
            result,
            Err(ScriptMetadataError::InvalidTargetSelector { selector }) if selector == "amiga"
        ));
    }

    #[test]
    fn invalid_match_spec_is_an_error() {
        let result = ScriptMetadata::from_source(
            "# /// script\n# [tool.conda]\n# dependencies = [\"python =!=3\"]\n# ///\n",
        );
        assert!(matches!(
            result,
            Err(ScriptMetadataError::InvalidMatchSpec { .. })
        ));
    }

    #[test]
    fn dependency_without_a_name_is_an_error() {
        let result = ScriptMetadata::from_source(
            "# /// script\n# [tool.conda]\n# dependencies = [\">=1.19\"]\n# ///\n",
        );
        assert!(matches!(
            result,
            Err(ScriptMetadataError::InvalidMatchSpec { .. })
        ));
    }

    #[test]
    fn unknown_keys_are_an_error() {
        let result = ScriptMetadata::from_source(
            "# /// script\n# [tool.pixi]\n# entry-point = \"python\"\n# ///\n",
        );
        assert!(matches!(result, Err(ScriptMetadataError::Toml(_))));
    }

    #[test]
    fn block_comments_are_not_supported() {
        // Only line comments can carry a metadata block; a block comment is
        // simply not recognized as one.
        let metadata = ScriptMetadata::from_source(
            "/* /// script\n[tool.conda]\ndependencies = [\"zlib\"]\n/// */\n",
        )
        .unwrap();
        assert!(metadata.is_none());
    }

    #[test]
    fn document_is_preserved_verbatim() {
        let metadata = ScriptMetadata::from_source(
            "# /// script\n# [tool.conda]\n# dependencies = [\"zlib\"]\n# ///\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            metadata.document(),
            "[tool.conda]\ndependencies = [\"zlib\"]"
        );
    }
}
