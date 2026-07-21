use std::path::{Path, PathBuf};

use miette::Diagnostic;
use thiserror::Error;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{
    TomlError, Warning, WorkspaceManifest, pyproject::PyProjectManifest, toml::FromTomlStr,
};

/// A Python script containing a PEP 723 metadata block.
#[derive(Debug, Clone)]
pub struct ScriptManifest {
    path: PathBuf,
    metadata: String,
    prelude: String,
    postlude: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ScriptWorkspaceConfig {
    pub channels_explicit: bool,
    pub platforms_explicit: bool,
}

impl ScriptManifest {
    /// Add a PEP 723 metadata block to a new or existing Python script.
    pub fn initialize(
        path: impl AsRef<Path>,
        channels: &[String],
    ) -> Result<Self, ScriptManifestError> {
        let path = std::path::absolute(path)?;
        script_name(&path)?;

        let contents = match fs_err::read(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        if ScriptBlock::parse(&contents)?.is_some() {
            return Err(ScriptManifestError::AlreadyInitialized { path });
        }

        let (bom, shebang, body) = extract_script_header(&contents)?;
        let mut metadata =
            "requires-python = \">=3.11\"\ndependencies = []\n\n[tool.pixi.workspace]\n"
                .parse::<DocumentMut>()
                .expect("the default script metadata is valid TOML");
        metadata["tool"]["pixi"]["workspace"]["channels"] =
            Item::Value(Value::Array(string_array(channels)));
        metadata["tool"]["pixi"]["dependencies"] = Item::Table(Table::new());

        let mut output = String::new();
        output.push_str(bom);
        if let Some(shebang) = shebang {
            output.push_str(shebang);
            output.push_str("\n#\n");
        }
        output.push_str(&serialize_metadata(&metadata.to_string()));
        if !body.is_empty() {
            output.push('\n');
            output.push_str(body);
        }

        fs_err::create_dir_all(
            path.parent()
                .expect("an absolute script path always has a parent"),
        )?;
        fs_err::write(&path, output)?;

        Ok(Self::from_path(path)?
            .expect("metadata serialized by the script initializer must be parseable"))
    }

    /// Read the PEP 723 metadata block from a script.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Option<Self>, ScriptManifestError> {
        let contents = fs_err::read(&path)?;
        let Some(block) = ScriptBlock::parse(&contents)? else {
            return Ok(None);
        };
        block.metadata.parse::<DocumentMut>()?;

        Ok(Some(Self {
            path: std::path::absolute(path)?,
            metadata: block.metadata,
            prelude: block.prelude,
            postlude: block.postlude,
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> &str {
        &self.metadata
    }

    pub fn metadata_document(&self) -> Result<DocumentMut, ScriptManifestError> {
        Ok(self.metadata.parse()?)
    }

    pub fn workspace_config(&self) -> Result<ScriptWorkspaceConfig, ScriptManifestError> {
        let metadata = self.metadata_document()?;
        let tool = metadata.get("tool").and_then(Item::as_table_like);
        let workspace = tool
            .and_then(|tool| tool.get("pixi"))
            .and_then(Item::as_table_like)
            .and_then(|pixi| pixi.get("workspace"))
            .and_then(Item::as_table_like);
        let conda = tool
            .and_then(|tool| tool.get("conda"))
            .and_then(Item::as_table_like);

        Ok(ScriptWorkspaceConfig {
            channels_explicit: conda.is_some_and(|table| table.contains_key("channels"))
                || workspace.is_some_and(|table| table.contains_key("channels")),
            platforms_explicit: workspace.is_some_and(|table| table.contains_key("platforms")),
        })
    }

    /// Parse the inline metadata using the same semantics as `pyproject.toml`.
    pub fn into_workspace_manifest(
        self,
    ) -> Result<(WorkspaceManifest, Vec<Warning>), ScriptManifestError> {
        let root_directory = self
            .path
            .parent()
            .expect("an absolute script path always has a parent");
        let project_name = script_name(&self.path)?;
        let pyproject = inline_pyproject(self.metadata(), project_name)?;
        let (workspace, package, warnings) =
            PyProjectManifest::from_toml_str(&pyproject.to_string())?
                .into_workspace_manifest(root_directory)?;

        debug_assert!(package.is_none(), "script manifests cannot define packages");
        Ok((workspace, warnings))
    }

    /// Replace the metadata block while preserving the Python around it.
    pub fn write_metadata(&self, metadata: &DocumentMut) -> Result<(), ScriptManifestError> {
        let contents = format!(
            "{}{}{}",
            self.prelude,
            serialize_metadata(&metadata.to_string()),
            self.postlude
        );
        fs_err::write(&self.path, contents)?;
        Ok(())
    }
}

fn string_array(values: &[String]) -> Array {
    let mut array = Array::new();
    array.extend(values.iter().map(String::as_str));
    array
}

fn extract_script_header(
    contents: &[u8],
) -> Result<(&str, Option<&str>, &str), ScriptManifestError> {
    let contents = std::str::from_utf8(contents)?;
    let (bom, contents) = contents
        .strip_prefix('\u{feff}')
        .map_or(("", contents), |contents| ("\u{feff}", contents));
    if !contents.starts_with("#!") {
        return Ok((bom, None, contents));
    }

    let bytes = contents.as_bytes();
    let end = bytes
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .unwrap_or(bytes.len());
    let newline_width = match bytes.get(end..) {
        Some([b'\r', b'\n', ..]) => 2,
        Some([b'\r' | b'\n', ..]) => 1,
        _ => 0,
    };

    Ok((
        bom,
        Some(&contents[..end]),
        &contents[end + newline_width..],
    ))
}

fn script_name(path: &Path) -> Result<&str, ScriptManifestError> {
    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ScriptManifestError::InvalidFilename {
            path: path.to_path_buf(),
        })
}

fn inline_pyproject(
    metadata: &str,
    project_name: &str,
) -> Result<DocumentMut, ScriptManifestError> {
    let mut metadata = metadata.parse::<DocumentMut>()?;
    validate_subset(&metadata)?;

    let dependencies = metadata
        .remove("dependencies")
        .unwrap_or_else(|| Item::Value(Value::Array(Array::new())));
    let requires_python = metadata.remove("requires-python");
    let mut tool = metadata
        .remove("tool")
        .map(|tool| {
            tool.into_table()
                .map_err(|_| ScriptManifestError::InvalidToolTable)
        })
        .transpose()?
        .unwrap_or_default();
    let pixi = tool
        .remove("pixi")
        .unwrap_or_else(|| Item::Table(Table::new()));
    if !pixi.is_table() {
        return Err(ScriptManifestError::InvalidPixiTable);
    }

    let mut pyproject = DocumentMut::new();
    pyproject["project"]["name"] = value(project_name);
    pyproject["project"]["dependencies"] = dependencies;
    if let Some(requires_python) = requires_python {
        pyproject["project"]["requires-python"] = requires_python;
    }
    pyproject["tool"]["pixi"] = pixi;

    ensure_pixi_workspace(&mut pyproject)?;
    Ok(pyproject)
}

fn ensure_pixi_workspace(pyproject: &mut DocumentMut) -> Result<(), ScriptManifestError> {
    if pyproject.get("tool").is_none() {
        pyproject["tool"] = Item::Table(Table::new());
    }
    if pyproject["tool"].get("pixi").is_none() {
        pyproject["tool"]["pixi"] = Item::Table(Table::new());
    }
    if pyproject["tool"]["pixi"].get("workspace").is_none() {
        pyproject["tool"]["pixi"]["workspace"] = Item::Table(Table::new());
    }
    if !pyproject["tool"]["pixi"]["workspace"].is_table() {
        return Err(ScriptManifestError::InvalidPixiWorkspace);
    }
    let workspace = pyproject["tool"]["pixi"]["workspace"]
        .as_table_mut()
        .expect("workspace was checked to be a table");
    for key in ["channels", "platforms"] {
        if !workspace.contains_key(key) {
            workspace.insert(key, Item::Value(Value::Array(Array::new())));
        }
    }
    Ok(())
}

fn validate_subset(metadata: &DocumentMut) -> Result<(), ScriptManifestError> {
    let unsupported_root = metadata
        .as_table()
        .iter()
        .map(|(key, _)| key)
        .filter(|key| !matches!(*key, "dependencies" | "requires-python" | "tool"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !unsupported_root.is_empty() {
        return Err(ScriptManifestError::UnsupportedFields(unsupported_root));
    }

    let Some(pixi) = metadata
        .get("tool")
        .and_then(Item::as_table_like)
        .and_then(|tool| tool.get("pixi"))
        .and_then(Item::as_table_like)
    else {
        return Ok(());
    };

    let mut unsupported = unsupported_keys(
        pixi,
        "tool.pixi",
        &[
            "activation",
            "constraints",
            "dependencies",
            "pypi-dependencies",
            "system-requirements",
            "target",
            "workspace",
        ],
    );

    if let Some(workspace) = pixi.get("workspace").and_then(Item::as_table_like) {
        unsupported.extend(unsupported_keys(
            workspace,
            "tool.pixi.workspace",
            &[
                "channel-priority",
                "channels",
                "platforms",
                "preview",
                "pypi-options",
                "requires-pixi",
                "solve-strategy",
            ],
        ));
    }

    if let Some(targets) = pixi.get("target").and_then(Item::as_table_like) {
        for (selector, target) in targets.iter() {
            let path = format!("tool.pixi.target.{selector}");
            let Some(target) = target.as_table_like() else {
                unsupported.push(path);
                continue;
            };
            unsupported.extend(unsupported_keys(
                target,
                &path,
                &[
                    "activation",
                    "constraints",
                    "dependencies",
                    "pypi-dependencies",
                ],
            ));
        }
    }

    if unsupported.is_empty() {
        Ok(())
    } else {
        unsupported.sort();
        unsupported.dedup();
        Err(ScriptManifestError::UnsupportedFields(unsupported))
    }
}

fn unsupported_keys(
    table: &dyn toml_edit::TableLike,
    prefix: &str,
    allowed: &[&str],
) -> Vec<String> {
    table
        .iter()
        .map(|(key, _)| key)
        .filter(|key| !allowed.contains(key))
        .map(|key| format!("{prefix}.{key}"))
        .collect()
}

#[derive(Debug, Error, Diagnostic)]
pub enum ScriptManifestError {
    #[error(transparent)]
    TomlEdit(#[from] toml_edit::TomlError),

    #[error(transparent)]
    Toml(#[from] TomlError),

    #[error("the script filename cannot be used as a project name: {}", path.display())]
    InvalidFilename { path: PathBuf },

    #[error("{} is already a PEP 723 script", path.display())]
    AlreadyInitialized { path: PathBuf },

    #[error("`tool.pixi.workspace` must be a table")]
    InvalidPixiWorkspace,

    #[error("`tool.pixi` must be a table")]
    InvalidPixiTable,

    #[error("`tool` must be a table")]
    InvalidToolTable,

    #[error("PEP 723 scripts do not support: {}", .0.join(", "))]
    #[diagnostic(help("A script represents one implicit default environment."))]
    UnsupportedFields(Vec<String>),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("the PEP 723 metadata block is not valid UTF-8")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("the opening `# /// script` marker has no closing `# ///` marker")]
    UnclosedBlock,

    #[error("the script contains multiple PEP 723 metadata blocks")]
    DuplicateBlock,
}

// Keep this envelope parser aligned with uv's `uv-scripts` implementation. The
// TOML model above remains Pixi-owned so script and pyproject semantics cannot drift.
struct ScriptBlock {
    prelude: String,
    metadata: String,
    postlude: String,
}

impl ScriptBlock {
    fn parse(contents: &[u8]) -> Result<Option<Self>, ScriptManifestError> {
        const OPENING: &[u8] = b"# /// script";
        const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
        let Some(index) = contents
            .windows(OPENING.len())
            .position(|window| window == OPENING)
        else {
            return Ok(None);
        };
        let follows_bom = index == UTF8_BOM.len() && contents.starts_with(UTF8_BOM);
        if index != 0 && !follows_bom && !matches!(contents[index - 1], b'\r' | b'\n') {
            return Ok(None);
        }

        let prelude = std::str::from_utf8(&contents[..index])?;
        let contents = std::str::from_utf8(&contents[index..])?;
        let mut lines = contents.split_inclusive('\n');
        let Some(opening) = lines.next() else {
            return Ok(None);
        };
        if without_line_ending(opening) != "# /// script" {
            return Ok(None);
        }

        let mut toml = Vec::new();
        let mut offset = opening.len();
        let mut line_end_offsets = Vec::new();
        for raw_line in lines {
            let line = without_line_ending(raw_line);
            let Some(line) = line.strip_prefix('#') else {
                break;
            };
            if line.is_empty() {
                toml.push("");
            } else if let Some(line) = line.strip_prefix(' ') {
                toml.push(line);
            } else {
                break;
            }
            offset += raw_line.len();
            line_end_offsets.push(offset);
        }

        let Some(reverse_index) = toml.iter().rev().position(|line| *line == "///") else {
            return Err(ScriptManifestError::UnclosedBlock);
        };
        let closing_index = toml.len() - reverse_index;
        let postlude = &contents[line_end_offsets[closing_index - 1]..];
        toml.truncate(closing_index - 1);

        reject_duplicate_block(&postlude.lines().collect::<Vec<_>>())?;

        Ok(Some(Self {
            prelude: prelude.to_owned(),
            metadata: toml.join("\n") + "\n",
            postlude: postlude.to_owned(),
        }))
    }
}

fn without_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn reject_duplicate_block(lines: &[&str]) -> Result<(), ScriptManifestError> {
    for (index, line) in lines.iter().enumerate() {
        if *line != "# /// script" {
            continue;
        }
        if lines[index + 1..]
            .iter()
            .take_while(|line| {
                line.strip_prefix('#')
                    .is_some_and(|content| content.is_empty() || content.starts_with(' '))
            })
            .any(|line| *line == "# ///")
        {
            return Err(ScriptManifestError::DuplicateBlock);
        }
    }
    Ok(())
}

fn serialize_metadata(metadata: &str) -> String {
    let mut output = String::with_capacity(metadata.len() + 32);
    output.push_str("# /// script\n");
    for line in metadata.lines() {
        output.push('#');
        if !line.is_empty() {
            output.push(' ');
            output.push_str(line);
        }
        output.push('\n');
    }
    output.push_str("# ///\n");
    output
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_pypi_spec::PypiPackageName;
    use rattler_conda_types::PackageName;
    use tempfile::TempDir;

    use super::*;
    use crate::SpecType;

    fn script(source: &str) -> (TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.py");
        fs_err::write(&path, source).unwrap();
        (directory, path)
    }

    #[test]
    fn initializes_a_script_without_replacing_its_python() {
        let (directory, path) = script("#!/usr/bin/env python\r\nprint('hello')\r\n");

        let script = ScriptManifest::initialize(&path, &["conda-forge".to_owned()]).unwrap();

        assert_eq!(script.path(), path);
        assert_eq!(
            fs_err::read_to_string(&path).unwrap(),
            r#"#!/usr/bin/env python
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
#
# [tool.pixi.dependencies]
# ///

print('hello')"#
                .to_owned()
                + "\r\n"
        );
        assert!(!directory.path().join("pixi.toml").exists());
    }

    #[test]
    fn initializing_preserves_a_utf8_bom_at_the_start_of_the_script() {
        let (_directory, path) = script("\u{feff}print('hello')\r\n");

        ScriptManifest::initialize(&path, &[]).unwrap();

        let contents = fs_err::read_to_string(&path).unwrap();
        assert!(contents.starts_with("\u{feff}# /// script\n"));
        assert_eq!(contents.matches('\u{feff}').count(), 1);
        assert!(contents.ends_with("\n\nprint('hello')\r\n"));

        assert!(matches!(
            ScriptManifest::initialize(&path, &[]),
            Err(ScriptManifestError::AlreadyInitialized { .. })
        ));
    }

    #[test]
    fn initializes_a_new_script_and_its_parent_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/example.py");

        ScriptManifest::initialize(&path, &[]).unwrap();

        assert_eq!(
            fs_err::read_to_string(path).unwrap(),
            r#"# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = []
#
# [tool.pixi.dependencies]
# ///
"#
        );
    }

    #[test]
    fn refuses_to_initialize_an_existing_script_manifest() {
        let (_directory, path) = script("# /// script\n# dependencies = []\n# ///\n");

        assert!(matches!(
            ScriptManifest::initialize(&path, &[]),
            Err(ScriptManifestError::AlreadyInitialized { .. })
        ));
    }

    #[test]
    fn parses_standard_and_pixi_dependencies_with_pyproject_semantics() {
        let (_directory, path) = script(
            r#"#!/usr/bin/env python
# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2"]
#
# [tool.conda]
# channels = "not interpreted by pixi"
# dependencies = ["missing >=1"]
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
# platforms = ["linux-64"]
#
# [tool.pixi.dependencies]
# python = "3.12.*"
# zlib = "*"
#
# [tool.pixi.pypi-dependencies]
# requests = "<3"
#
# [tool.some-future-runner]
# option = true
# ///
print("hello")
"#,
        );

        let script = ScriptManifest::from_path(path).unwrap().unwrap();
        let (manifest, warnings) = script.into_workspace_manifest().unwrap();
        assert!(warnings.is_empty());
        assert_eq!(manifest.workspace.name.as_deref(), Some("example"));
        assert_eq!(
            manifest
                .workspace
                .platforms
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["linux-64"]
        );
        assert_eq!(
            manifest
                .workspace
                .channels
                .iter()
                .map(|channel| channel.channel.to_string())
                .collect::<Vec<_>>(),
            ["conda-forge"]
        );

        let target = manifest.default_feature().targets.default();
        let python = PackageName::from_str("python").unwrap();
        let python_specs = target
            .run_dependencies()
            .unwrap()
            .get(&python)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(python_specs, ["3.12.*"]);
        assert!(target.has_dependency(
            &PackageName::from_str("zlib").unwrap(),
            SpecType::Run,
            None
        ));
        assert!(!target.has_dependency(
            &PackageName::from_str("missing").unwrap(),
            SpecType::Run,
            None
        ));
        assert_eq!(
            target
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .get(&PypiPackageName::from_str("requests").unwrap())
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn resolves_relative_paths_from_the_script_directory() {
        let (directory, path) = script(
            r#"# /// script
# dependencies = ["demo @ ./demo"]
# ///
"#,
        );
        fs_err::create_dir(directory.path().join("demo")).unwrap();

        let script = ScriptManifest::from_path(path).unwrap().unwrap();
        let (manifest, _) = script.into_workspace_manifest().unwrap();
        let dependency = manifest
            .default_feature()
            .targets
            .default()
            .pypi_dependencies
            .as_ref()
            .unwrap()
            .get_single(&PypiPackageName::from_str("demo").unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(
            dependency.source.as_path(),
            Some(&directory.path().join("demo"))
        );
    }

    #[test]
    fn an_empty_standard_script_gets_one_implicit_workspace() {
        let (_directory, path) = script(
            r#"# /// script
# dependencies = []
# ///
print("hello")
"#,
        );

        let script = ScriptManifest::from_path(path).unwrap().unwrap();
        let (manifest, _) = script.into_workspace_manifest().unwrap();

        assert_eq!(manifest.workspace.name.as_deref(), Some("example"));
        assert_eq!(manifest.all_features().count(), 1);
        assert_eq!(manifest.environments.iter().count(), 1);
    }

    #[test]
    fn rejects_workspace_only_concepts() {
        let (_directory, path) = script(
            r#"# /// script
# dependencies = []
#
# [tool.pixi.target.linux-64.tasks]
# test = "pytest"
#
# [tool.pixi.feature.test.dependencies]
# pytest = "*"
#
# [tool.pixi.target.linux-64.host-dependencies]
# python = "*"
# ///
"#,
        );

        let error = ScriptManifest::from_path(path)
            .unwrap()
            .unwrap()
            .into_workspace_manifest()
            .unwrap_err();
        let ScriptManifestError::UnsupportedFields(fields) = error else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(
            fields,
            [
                "tool.pixi.feature",
                "tool.pixi.target.linux-64.host-dependencies",
                "tool.pixi.target.linux-64.tasks"
            ]
        );
    }

    #[test]
    fn rejects_unknown_pixi_fields_with_an_explicit_allowlist() {
        let (_directory, path) = script(
            r#"# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = []
# platforms = []
# description = "not execution metadata"
#
# [tool.pixi.target.linux-64.tasks]
# test = "pytest"
# ///
"#,
        );

        let error = ScriptManifest::from_path(path)
            .unwrap()
            .unwrap()
            .into_workspace_manifest()
            .unwrap_err();
        let ScriptManifestError::UnsupportedFields(fields) = error else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(
            fields,
            [
                "tool.pixi.target.linux-64.tasks",
                "tool.pixi.workspace.description"
            ]
        );
    }

    #[test]
    fn rejects_invalid_and_duplicate_blocks() {
        let (_directory, unclosed) = script(
            r#"# /// script
# dependencies = []
print("hello")
"#,
        );
        assert!(matches!(
            ScriptManifest::from_path(unclosed),
            Err(ScriptManifestError::UnclosedBlock)
        ));

        let (_directory, duplicate) = script(
            r#"# /// script
# dependencies = []
# ///
print("first")
# /// script
# dependencies = []
# ///
"#,
        );
        assert!(matches!(
            ScriptManifest::from_path(duplicate),
            Err(ScriptManifestError::DuplicateBlock)
        ));

        let (_directory, pixi_script) = script(
            r#"# /// pixi-script
# [dependencies]
# requests = "*"
# ///
"#,
        );
        assert!(
            ScriptManifest::from_path(pixi_script).unwrap().is_none(),
            "a future Pixi-native block must not be interpreted as PEP 723"
        );
    }

    #[test]
    fn metadata_edits_preserve_the_python_and_other_tools() {
        let (_directory, path) = script(
            r#"#!/usr/bin/env -S uv run --script
# /// script
# dependencies = ["requests"]
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["numpy"]
# ///

print("hello")
"#,
        );
        let script = ScriptManifest::from_path(&path).unwrap().unwrap();
        let mut metadata = script.metadata_document().unwrap();
        metadata["dependencies"]
            .as_array_mut()
            .unwrap()
            .push("rich");

        script.write_metadata(&metadata).unwrap();

        assert_eq!(
            fs_err::read_to_string(path).unwrap(),
            r#"#!/usr/bin/env -S uv run --script
# /// script
# dependencies = ["requests", "rich"]
#
# [tool.uv]
# prerelease = "allow"
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["numpy"]
# ///

print("hello")
"#
        );
    }
}
