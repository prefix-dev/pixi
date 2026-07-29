use std::{
    error::Error,
    fmt,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode, SourceSpan};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::{
    TomlError, Warning, WorkspaceManifest,
    pyproject::PyProjectManifest,
    toml::{FromTomlStr, TomlDocument},
};

/// A Python script containing a PEP 723 metadata block.
#[derive(Debug, Clone)]
pub struct ScriptManifest {
    path: PathBuf,
    metadata: String,
    prelude: String,
    postlude: String,
    line_ending: LineEnding,
    source_map: ScriptSourceMap,
}

#[derive(Debug, Clone)]
struct MetadataLine {
    metadata_start: usize,
    source_start: usize,
    len: usize,
}

#[derive(Debug, Clone)]
struct ScriptSourceMap {
    source: Arc<str>,
    opening: Range<usize>,
    metadata_lines: Vec<MetadataLine>,
}

impl ScriptSourceMap {
    fn metadata_offset(&self, offset: usize) -> usize {
        let Some(line) = self
            .metadata_lines
            .iter()
            .rev()
            .find(|line| line.metadata_start <= offset)
        else {
            return self.opening.start;
        };
        line.source_start + offset.saturating_sub(line.metadata_start).min(line.len)
    }

    fn span(&self, offset: usize, len: usize, synthetic_prefix: usize) -> SourceSpan {
        let Some(metadata_start) = offset.checked_sub(synthetic_prefix) else {
            return SourceSpan::from(self.opening.clone());
        };
        let metadata_end = offset.saturating_add(len).saturating_sub(synthetic_prefix);
        let start = self.metadata_offset(metadata_start);
        let end = self.metadata_offset(metadata_end).max(start);
        SourceSpan::new(start.into(), end - start)
    }
}

#[derive(Debug)]
pub struct ScriptMetadataError {
    error: TomlError,
    source: NamedSource<Arc<str>>,
    source_map: ScriptSourceMap,
    synthetic_prefix: usize,
}

impl fmt::Display for ScriptMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl Error for ScriptMetadataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

impl Diagnostic for ScriptMetadataError {
    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.error.help()
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(self.error.labels()?.map(|label| {
            let span = self
                .source_map
                .span(label.offset(), label.len(), self.synthetic_prefix);
            let text = label.label().map(str::to_owned);
            if label.primary() {
                LabeledSpan::new_primary_with_span(text, span)
            } else {
                LabeledSpan::new_with_span(text, span)
            }
        })))
    }
}

/// An editable PEP 723 manifest presented using Pixi's pyproject semantics.
///
/// The TOML document is synthetic: it exposes the inline metadata through the
/// same shape as a `pyproject.toml`. Rendering converts it back into a PEP 723
/// block while preserving the surrounding Python source.
#[derive(Debug, Clone)]
pub struct ScriptManifestDocument {
    script: ScriptManifest,
    document: TomlDocument,
}

impl ScriptManifestDocument {
    pub fn new(script: ScriptManifest) -> Result<Self, ScriptManifestError> {
        let document = TomlDocument::new(script.pyproject_document()?);
        Ok(Self { script, document })
    }

    pub fn document(&self) -> &TomlDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut TomlDocument {
        &mut self.document
    }

    pub fn render(&self) -> Result<String, ScriptManifestError> {
        self.script
            .render_pyproject_document(self.document.as_document())
    }
}

impl fmt::Display for ScriptManifestDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render().map_err(|_| fmt::Error)?)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScriptWorkspaceConfig {
    pub channels_explicit: bool,
    pub platforms_explicit: bool,
}

#[derive(Debug, Clone, Copy)]
enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    fn detect(contents: &[u8]) -> Self {
        if contents.windows(2).any(|window| window == b"\r\n") {
            Self::CrLf
        } else {
            Self::Lf
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
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

        let line_ending = LineEnding::detect(&contents).as_str();
        let (bom, shebang, body) = extract_script_header(&contents)?;
        let mut metadata = "requires-python = \">=3.11\"\ndependencies = []\n"
            .parse::<DocumentMut>()
            .expect("the default script metadata is valid TOML");
        if !channels.is_empty() {
            ensure_metadata_tool_table(&mut metadata)?;
            metadata["tool"]["pixi"] = Item::Table(implicit_table());
            let mut workspace = Table::new();
            workspace["channels"] = Item::Value(Value::Array(string_array(channels)));
            metadata["tool"]["pixi"]
                .as_table_mut()
                .expect("pixi was initialized as a table")
                .insert("workspace", Item::Table(workspace));
        }

        let mut output = String::new();
        output.push_str(bom);
        if let Some(shebang) = shebang {
            output.push_str(shebang);
            output.push_str(line_ending);
            output.push('#');
            output.push_str(line_ending);
        }
        output.push_str(&serialize_metadata(&metadata.to_string(), line_ending));
        if !body.is_empty() {
            output.push_str(line_ending);
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
        Self::from_source(path, &contents)
    }

    /// Parse a PEP 723 metadata block from Python source at a given path.
    pub fn from_source(
        path: impl AsRef<Path>,
        contents: &[u8],
    ) -> Result<Option<Self>, ScriptManifestError> {
        let Some(block) = ScriptBlock::parse(contents)? else {
            return Ok(None);
        };
        let path = std::path::absolute(path)?;
        block.metadata.parse::<DocumentMut>().map_err(|error| {
            ScriptManifestError::Metadata(Box::new(ScriptMetadataError {
                error: error.into(),
                source: NamedSource::new(
                    path.to_string_lossy(),
                    Arc::clone(&block.source_map.source),
                ),
                source_map: block.source_map.clone(),
                synthetic_prefix: 0,
            }))
        })?;

        Ok(Some(Self {
            path,
            metadata: block.metadata,
            prelude: block.prelude,
            postlude: block.postlude,
            line_ending: block.line_ending,
            source_map: block.source_map,
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> &str {
        &self.metadata
    }

    pub fn metadata_document(&self) -> Result<DocumentMut, ScriptManifestError> {
        self.metadata
            .parse()
            .map_err(|error: toml_edit::TomlError| self.metadata_error(error.into(), 0))
    }

    /// Present the script metadata as a synthetic pyproject document for Pixi's editors.
    pub fn pyproject_document(&self) -> Result<DocumentMut, ScriptManifestError> {
        Ok(inline_pyproject(self, script_name(&self.path)?)?.document)
    }

    pub fn workspace_config(&self) -> Result<ScriptWorkspaceConfig, ScriptManifestError> {
        let metadata = self.metadata_document()?;
        let tool = metadata.get("tool").and_then(Item::as_table_like);
        let workspace = tool
            .and_then(|tool| tool.get("pixi"))
            .and_then(Item::as_table_like)
            .and_then(|pixi| pixi.get("workspace"))
            .and_then(Item::as_table_like);
        Ok(ScriptWorkspaceConfig {
            channels_explicit: workspace.is_some_and(|table| table.contains_key("channels")),
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
        let pyproject = inline_pyproject(&self, project_name)?;
        let pyproject_manifest = PyProjectManifest::from_toml_str(&pyproject.document.to_string())
            .map_err(|error| self.metadata_error(error, pyproject.metadata_offset))?;
        let (workspace, package, warnings) = pyproject_manifest
            .into_workspace_manifest(root_directory)
            .map_err(|error| self.metadata_error(error, pyproject.metadata_offset))?;

        debug_assert!(package.is_none(), "script manifests cannot define packages");
        Ok((workspace, warnings))
    }

    fn metadata_error(&self, error: TomlError, synthetic_prefix: usize) -> ScriptManifestError {
        ScriptManifestError::Metadata(Box::new(ScriptMetadataError {
            error,
            source: NamedSource::new(
                self.path.to_string_lossy(),
                Arc::clone(&self.source_map.source),
            ),
            source_map: self.source_map.clone(),
            synthetic_prefix,
        }))
    }

    /// Replace the metadata block while preserving the Python around it.
    pub fn write_metadata(&self, metadata: &DocumentMut) -> Result<(), ScriptManifestError> {
        let contents = format!(
            "{}{}{}",
            self.prelude,
            serialize_metadata(&metadata.to_string(), self.line_ending.as_str()),
            self.postlude
        );
        fs_err::write(&self.path, contents)?;
        Ok(())
    }

    /// Render edits to the synthetic pyproject back into the inline metadata block.
    pub fn render_pyproject_document(
        &self,
        pyproject: &DocumentMut,
    ) -> Result<String, ScriptManifestError> {
        let mut pyproject = pyproject.clone();
        let mut project = pyproject
            .remove("project")
            .and_then(|item| item.into_table().ok())
            .ok_or(ScriptManifestError::InvalidEditableDocument)?;
        let dependencies = project
            .remove("dependencies")
            .unwrap_or_else(|| Item::Value(Value::Array(Array::new())));

        let mut metadata = self.metadata_document()?;
        let uses_pixi_metadata = metadata
            .get("tool")
            .and_then(Item::as_table_like)
            .and_then(|tool| tool.get("pixi"))
            .and_then(Item::as_table_like)
            .is_some_and(|pixi| {
                pixi.contains_key("workspace") || pixi.contains_key("dependencies")
            });
        metadata["dependencies"] = dependencies;
        if let Some(requires_python) = project.remove("requires-python") {
            metadata["requires-python"] = requires_python;
        } else {
            metadata.remove("requires-python");
        }

        let updated_pixi = pyproject
            .get_mut("tool")
            .and_then(Item::as_table_like_mut)
            .and_then(|tool| tool.get_mut("pixi"))
            .and_then(Item::as_table_like_mut);
        let (updated_conda, updated_pypi) = if let Some(pixi) = updated_pixi {
            (
                pixi.remove("dependencies")
                    .and_then(|item| item.into_table().ok()),
                pixi.remove("pypi-dependencies")
                    .and_then(|item| item.into_table().ok()),
            )
        } else {
            (None, None)
        };
        sync_pixi_dependency_table(
            &mut metadata,
            updated_conda.or_else(|| uses_pixi_metadata.then(Table::new)),
            "dependencies",
        )?;
        sync_pixi_dependency_table(&mut metadata, updated_pypi, "pypi-dependencies")?;

        Ok(format!(
            "{}{}{}",
            self.prelude,
            serialize_metadata(&metadata.to_string(), self.line_ending.as_str()),
            self.postlude
        ))
    }
}

fn ensure_metadata_tool_table(metadata: &mut DocumentMut) -> Result<(), ScriptManifestError> {
    if metadata.get("tool").is_none() {
        metadata["tool"] = Item::Table(implicit_table());
    }
    if !metadata["tool"].is_table() {
        return Err(ScriptManifestError::InvalidToolTable);
    }
    Ok(())
}

fn sync_pixi_dependency_table(
    metadata: &mut DocumentMut,
    updated: Option<Table>,
    key: &'static str,
) -> Result<(), ScriptManifestError> {
    if let Some(updated) = updated {
        ensure_metadata_tool_table(metadata)?;
        if metadata["tool"].get("pixi").is_none() {
            metadata["tool"]["pixi"] = Item::Table(implicit_table());
        }
        let pixi = metadata["tool"]["pixi"]
            .as_table_mut()
            .ok_or(ScriptManifestError::InvalidPixiTable)?;
        pixi.insert(key, Item::Table(updated));
    } else if let Some(pixi) = metadata
        .get_mut("tool")
        .and_then(Item::as_table_like_mut)
        .and_then(|tool| tool.get_mut("pixi"))
        .and_then(Item::as_table_mut)
    {
        pixi.remove(key);
        if pixi.is_empty() {
            metadata["tool"]
                .as_table_mut()
                .expect("tool was checked to be a table")
                .remove("pixi");
        }
    }
    Ok(())
}

fn implicit_table() -> Table {
    let mut table = Table::new();
    table.set_implicit(true);
    table
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

struct InlinePyProject {
    document: DocumentMut,
    metadata_offset: usize,
}

fn inline_pyproject(
    script: &ScriptManifest,
    project_name: &str,
) -> Result<InlinePyProject, ScriptManifestError> {
    let metadata = script.metadata();
    let metadata_document = script.metadata_document()?;
    validate_subset(&metadata_document)?;

    let mut prefix = format!("[project]\nname = {}\n", Value::from(project_name));
    if !metadata_document.contains_key("dependencies") {
        prefix.push_str("dependencies = []\n");
    }
    let metadata_offset = prefix.len();
    let mut pyproject = format!("{prefix}{metadata}")
        .parse::<DocumentMut>()
        .map_err(|error| script.metadata_error(error.into(), metadata_offset))?;

    ensure_pixi_workspace(&mut pyproject)?;
    Ok(InlinePyProject {
        document: pyproject,
        metadata_offset,
    })
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
    #[diagnostic(transparent)]
    Metadata(#[from] Box<ScriptMetadataError>),

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

    #[error("the editable script document is missing its project table")]
    InvalidEditableDocument,

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
    line_ending: LineEnding,
    source_map: ScriptSourceMap,
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

        let line_ending = LineEnding::detect(contents);
        let source = Arc::<str>::from(std::str::from_utf8(contents)?);
        let prelude = &source[..index];
        let script_source = &source[index..];
        let mut lines = script_source.split_inclusive('\n');
        let Some(opening) = lines.next() else {
            return Ok(None);
        };
        if without_line_ending(opening) != "# /// script" {
            return Ok(None);
        }

        let mut toml = Vec::new();
        let mut metadata_lines = Vec::new();
        let mut metadata_len = 0;
        let mut offset = opening.len();
        let mut line_end_offsets = Vec::new();
        for raw_line in lines {
            let line = without_line_ending(raw_line);
            let Some(line) = line.strip_prefix('#') else {
                break;
            };
            let (line, prefix_len) = if line.is_empty() {
                ("", 1)
            } else if let Some(line) = line.strip_prefix(' ') {
                (line, 2)
            } else {
                break;
            };
            toml.push(line);
            metadata_lines.push(MetadataLine {
                metadata_start: metadata_len,
                source_start: index + offset + prefix_len,
                len: line.len(),
            });
            metadata_len += line.len() + 1;
            offset += raw_line.len();
            line_end_offsets.push(offset);
        }

        let Some(reverse_index) = toml.iter().rev().position(|line| *line == "///") else {
            return Err(ScriptManifestError::UnclosedBlock);
        };
        let closing_index = toml.len() - reverse_index;
        let postlude = &script_source[line_end_offsets[closing_index - 1]..];
        toml.truncate(closing_index - 1);
        metadata_lines.truncate(closing_index - 1);

        reject_duplicate_block(&postlude.lines().collect::<Vec<_>>())?;
        let opening = index..index + without_line_ending(opening).len();

        Ok(Some(Self {
            prelude: prelude.to_owned(),
            metadata: toml.join("\n") + "\n",
            postlude: postlude.to_owned(),
            line_ending,
            source_map: ScriptSourceMap {
                source,
                opening,
                metadata_lines,
            },
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

fn serialize_metadata(metadata: &str, line_ending: &str) -> String {
    let mut output = String::with_capacity(metadata.len() + 32);
    output.push_str("# /// script");
    output.push_str(line_ending);
    for line in metadata.lines() {
        output.push('#');
        if !line.is_empty() {
            output.push(' ');
            output.push_str(line);
        }
        output.push_str(line_ending);
    }
    output.push_str("# ///");
    output.push_str(line_ending);
    output
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_pypi_spec::PypiPackageName;
    use pixi_test_utils::format_diagnostic;
    use rattler_conda_types::PackageName;
    use tempfile::TempDir;
    use toml_edit::value;

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
        let expected = r#"#!/usr/bin/env python
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
# ///

print('hello')
"#;
        assert_eq!(
            fs_err::read_to_string(&path).unwrap(),
            expected.replace('\n', "\r\n")
        );
        assert!(!directory.path().join("pixi.toml").exists());
    }

    #[test]
    fn initializing_preserves_a_utf8_bom_at_the_start_of_the_script() {
        let (_directory, path) = script("\u{feff}print('hello')\r\n");

        ScriptManifest::initialize(&path, &[]).unwrap();

        let contents = fs_err::read_to_string(&path).unwrap();
        assert!(contents.starts_with("\u{feff}# /// script\r\n"));
        assert_eq!(contents.matches('\u{feff}').count(), 1);
        assert!(contents.ends_with("\r\n\r\nprint('hello')\r\n"));
        assert!(!contents.replace("\r\n", "").contains('\n'));

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

    #[test]
    fn metadata_edits_preserve_crlf_line_endings() {
        let (_directory, path) = script(
            &r#"# /// script
# dependencies = []
# ///
print("hello")
"#
            .replace('\n', "\r\n"),
        );
        let script = ScriptManifest::from_path(&path).unwrap().unwrap();
        let mut metadata = script.metadata_document().unwrap();
        metadata["dependencies"]
            .as_array_mut()
            .unwrap()
            .push("requests");

        script.write_metadata(&metadata).unwrap();

        let contents = fs_err::read_to_string(path).unwrap();
        assert!(!contents.replace("\r\n", "").contains('\n'));
        insta::assert_snapshot!(contents.replace("\r\n", "\n"), @r###"
        # /// script
        # dependencies = ["requests"]
        # ///
        print("hello")
        "###);
    }

    #[test]
    fn syntax_errors_point_into_the_python_script() {
        let error = ScriptManifest::from_source(
            "/example.py",
            br#"print("before")
# /// script
# dependencies = ["requests"
# ///
print("after")
"#,
        )
        .unwrap_err();
        let diagnostic = format_diagnostic(&error);

        assert!(diagnostic.contains("[/example.py:3:"));
        assert!(diagnostic.contains(r#"# dependencies = ["requests""#));
        assert!(!diagnostic.contains("[project]"));
    }

    #[test]
    fn semantic_errors_point_into_the_python_script() {
        let script = ScriptManifest::from_source(
            "/example.py",
            br#"print("before")
# /// script
# dependencies = []
#
# [tool.pixi.dependencies]
# bad-package = "!invalid!"
# ///
print("after")
"#,
        )
        .unwrap()
        .unwrap();
        let error = script.into_workspace_manifest().unwrap_err();
        let diagnostic = format_diagnostic(&error);

        assert!(diagnostic.contains("[/example.py:6:"));
        assert!(diagnostic.contains(r#"# bad-package = "!invalid!""#));
        assert!(!diagnostic.contains("[project]"));
    }

    #[test]
    fn pyproject_edits_use_pep_for_pypi_and_pixi_for_conda() {
        let (_directory, path) = script(
            r#"# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.conda]
# custom = "preserved"
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
#
# [tool.pixi.dependencies]
# python = ">=3.11"
# openssl = { version = ">=3", channel = "conda-forge" }
# ///
print("hello")
"#,
        );
        let script = ScriptManifest::from_path(&path).unwrap().unwrap();
        let mut pyproject = script.pyproject_document().unwrap();
        pyproject["project"]["dependencies"]
            .as_array_mut()
            .unwrap()
            .push("requests>=2");
        pyproject["tool"]["pixi"]["dependencies"]["numpy"] = value(">=2");

        let rendered = script.render_pyproject_document(&pyproject).unwrap();
        fs_err::write(&path, rendered).unwrap();
        let metadata = ScriptManifest::from_path(path)
            .unwrap()
            .unwrap()
            .metadata_document()
            .unwrap();

        assert_eq!(
            metadata["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>(),
            ["requests>=2"]
        );
        assert_eq!(
            metadata["tool"]["conda"]["custom"].as_str(),
            Some("preserved")
        );
        let conda = metadata["tool"]["pixi"]["dependencies"].as_table().unwrap();
        assert!(conda.contains_key("python"));
        assert!(conda.contains_key("openssl"));
        assert!(conda.contains_key("numpy"));
    }

    #[test]
    fn pyproject_edits_preserve_empty_pixi_dependency_tables() {
        let (_directory, path) = script(
            r#"# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2"]
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
#
# [tool.pixi.dependencies]
# ///
"#,
        );
        let script = ScriptManifest::from_path(&path).unwrap().unwrap();
        let mut pyproject = script.pyproject_document().unwrap();
        pyproject["project"]["dependencies"]
            .as_array_mut()
            .unwrap()
            .clear();
        pyproject["tool"]["pixi"]
            .as_table_mut()
            .unwrap()
            .remove("dependencies");

        let rendered = script.render_pyproject_document(&pyproject).unwrap();
        fs_err::write(&path, rendered).unwrap();
        let metadata = ScriptManifest::from_path(path)
            .unwrap()
            .unwrap()
            .metadata_document()
            .unwrap();

        assert!(metadata["dependencies"].as_array().unwrap().is_empty());
        assert!(
            metadata["tool"]["pixi"]["dependencies"]
                .as_table()
                .unwrap()
                .is_empty()
        );
        assert_eq!(metadata["requires-python"].as_str(), Some(">=3.11"));
    }
}
