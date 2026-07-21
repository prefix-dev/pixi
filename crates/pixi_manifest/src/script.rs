use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
};

use miette::Diagnostic;
use pixi_pypi_spec::PypiPackageName;
use pixi_spec::PixiSpec;
use rattler_conda_types::{ChannelConfig, MatchSpec, PackageName, ParseStrictness, VersionSpec};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{
    InternalDependencyBehavior, SpecType, TomlError, Warning, WorkspaceManifest,
    pyproject::PyProjectManifest, toml::FromTomlStr,
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
        let mut metadata = "requires-python = \">=3.11\"\ndependencies = []\n\n[tool.conda]\n"
            .parse::<DocumentMut>()
            .expect("the default script metadata is valid TOML");
        metadata["tool"]["conda"]["channels"] = Item::Value(Value::Array(string_array(channels)));
        metadata["tool"]["conda"]["dependencies"] = Item::Value(Value::Array(Array::new()));

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

    /// Present the script metadata as a synthetic pyproject document for Pixi's editors.
    pub fn pyproject_document(&self) -> Result<DocumentMut, ScriptManifestError> {
        let root_directory = self
            .path
            .parent()
            .expect("an absolute script path always has a parent");
        Ok(inline_pyproject(self.metadata(), script_name(&self.path)?, root_directory)?.0)
    }

    pub fn workspace_config(&self) -> Result<ScriptWorkspaceConfig, ScriptManifestError> {
        let metadata = self.metadata_document()?;
        let workspace = nested_table(&metadata, &["tool", "pixi", "workspace"]);
        let conda = nested_table(&metadata, &["tool", "conda"]);

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
        let (pyproject, has_explicit_python) =
            inline_pyproject(self.metadata(), project_name, root_directory)?;
        let requires_python = pyproject
            .get("project")
            .and_then(Item::as_table_like)
            .and_then(|project| project.get("requires-python"))
            .and_then(Item::as_str)
            .map(str::to_owned);
        let (mut workspace, package, warnings) =
            PyProjectManifest::from_toml_str(&pyproject.to_string())?
                .into_workspace_manifest(root_directory)?;

        debug_assert!(package.is_none(), "script manifests cannot define packages");
        if has_explicit_python && let Some(requires_python) = requires_python {
            let version = requires_python
                .strip_prefix("==")
                .unwrap_or(&requires_python)
                .parse::<VersionSpec>()?;
            workspace
                .default_feature_mut()
                .targets
                .default_mut()
                .add_dependency(
                    &PackageName::from_str("python").expect("python is a valid package name"),
                    &PixiSpec::from(version),
                    SpecType::Run,
                    InternalDependencyBehavior::Append,
                );
        }
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

    /// Render edits to the synthetic pyproject back into the inline metadata block.
    pub fn render_pyproject_document(
        &self,
        pyproject: &DocumentMut,
    ) -> Result<String, ScriptManifestError> {
        let root_directory = self
            .path
            .parent()
            .expect("an absolute script path always has a parent");
        let mut pyproject = pyproject.clone();
        let mut project = pyproject
            .remove("project")
            .and_then(|item| item.into_table().ok())
            .ok_or(ScriptManifestError::InvalidEditableDocument)?;
        let dependencies = project
            .remove("dependencies")
            .unwrap_or_else(|| Item::Value(Value::Array(Array::new())));

        let mut metadata = self.metadata_document()?;
        let rich_conda_names = nested_table(&metadata, &["tool", "pixi", "dependencies"])
            .map(conda_dependency_names)
            .transpose()?
            .unwrap_or_default();
        let edited_conda_names = nested_table(&pyproject, &["tool", "pixi", "dependencies"])
            .map(conda_dependency_names)
            .transpose()?
            .unwrap_or_default();

        let (updated_workspace, _, _) = PyProjectManifest::from_toml_str(&pyproject.to_string())?
            .into_workspace_manifest(root_directory)?;
        let channel_config = ChannelConfig::default_with_root_dir(root_directory.to_owned());
        let mut portable_conda = Array::new();
        if let Some(run_dependencies) = updated_workspace.default_feature().run_dependencies(None) {
            for (name, specs) in run_dependencies.iter() {
                if rich_conda_names.contains(name.as_normalized())
                    || !edited_conda_names.contains(name.as_normalized())
                {
                    continue;
                }
                let spec = specs
                    .first()
                    .expect("dependency maps never contain an empty spec set")
                    .clone()
                    .to_match_spec(name, &channel_config)?;
                portable_conda.push(spec.to_string());
            }
        }

        metadata["dependencies"] = dependencies;
        if let Some(requires_python) = project.remove("requires-python") {
            metadata["requires-python"] = requires_python;
        } else {
            metadata.remove("requires-python");
        }
        ensure_metadata_tool_table(&mut metadata)?;
        if metadata["tool"].get("conda").is_none() {
            metadata["tool"]["conda"] = Item::Table(Table::new());
        }
        metadata["tool"]["conda"]["dependencies"] = Item::Value(Value::Array(portable_conda));

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
        sync_pixi_dependency_table(&mut metadata, updated_conda, "dependencies", |name| {
            !rich_conda_names.contains(name)
        })?;
        sync_pixi_dependency_table(&mut metadata, updated_pypi, "pypi-dependencies", |_| false)?;

        Ok(format!(
            "{}{}{}",
            self.prelude,
            serialize_metadata(&metadata.to_string()),
            self.postlude
        ))
    }
}

fn ensure_metadata_tool_table(metadata: &mut DocumentMut) -> Result<(), ScriptManifestError> {
    if metadata.get("tool").is_none() {
        metadata["tool"] = Item::Table(Table::new());
    }
    if !metadata["tool"].is_table() {
        return Err(ScriptManifestError::InvalidToolTable);
    }
    Ok(())
}

fn sync_pixi_dependency_table(
    metadata: &mut DocumentMut,
    mut updated: Option<Table>,
    key: &'static str,
    remove: impl Fn(&str) -> bool,
) -> Result<(), ScriptManifestError> {
    if let Some(table) = &mut updated {
        let keys = table
            .iter()
            .filter_map(|(name, _)| {
                let normalized = PackageName::from_str(name).ok()?;
                remove(normalized.as_normalized()).then(|| name.to_owned())
            })
            .collect::<Vec<_>>();
        for name in keys {
            table.remove(&name);
        }
    }

    ensure_metadata_tool_table(metadata)?;
    if metadata["tool"].get("pixi").is_none() {
        metadata["tool"]["pixi"] = Item::Table(Table::new());
    }
    let pixi = metadata["tool"]["pixi"]
        .as_table_mut()
        .ok_or(ScriptManifestError::ExpectedTable("tool.pixi"))?;
    if let Some(updated) = updated.filter(|table| !table.is_empty()) {
        pixi.insert(key, Item::Table(updated));
    } else {
        pixi.remove(key);
    }
    if pixi.is_empty() {
        metadata["tool"]
            .as_table_mut()
            .expect("tool was checked to be a table")
            .remove("pixi");
    }
    Ok(())
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
    root_directory: &Path,
) -> Result<(DocumentMut, bool), ScriptManifestError> {
    let mut metadata = metadata.parse::<DocumentMut>()?;
    validate_subset(&metadata)?;

    let dependencies = metadata
        .remove("dependencies")
        .unwrap_or_else(|| Item::Value(Value::Array(Array::new())));
    let portable_pypi_names = portable_pypi_dependency_names(&dependencies, root_directory)?;
    let requires_python = metadata.remove("requires-python");
    let mut tool = metadata
        .remove("tool")
        .map(|tool| {
            tool.into_table()
                .map_err(|_| ScriptManifestError::InvalidToolTable)
        })
        .transpose()?
        .unwrap_or_default();
    let mut pixi = tool
        .remove("pixi")
        .unwrap_or_else(|| Item::Table(Table::new()));
    let conda = tool.remove("conda");
    let has_explicit_python =
        project_conda_metadata(&mut pixi, conda, root_directory, &portable_pypi_names)?;

    let mut pyproject = DocumentMut::new();
    pyproject["project"]["name"] = value(project_name);
    pyproject["project"]["dependencies"] = dependencies;
    if let Some(requires_python) = requires_python {
        pyproject["project"]["requires-python"] = requires_python;
    }
    pyproject["tool"]["pixi"] = pixi;

    ensure_pixi_workspace(&mut pyproject)?;
    Ok((pyproject, has_explicit_python))
}

/// Project the portable conda-exec metadata into Pixi's richer internal model.
/// The script itself remains unchanged; this only builds the synthetic
/// `pyproject.toml` consumed by the existing manifest conversion pipeline.
fn project_conda_metadata(
    pixi: &mut Item,
    conda: Option<Item>,
    root_directory: &Path,
    portable_pypi_names: &HashSet<String>,
) -> Result<bool, ScriptManifestError> {
    let pixi = pixi
        .as_table_mut()
        .ok_or(ScriptManifestError::InvalidPixiTable)?;

    let mut conda = conda
        .map(|item| {
            item.into_table()
                .map_err(|_| ScriptManifestError::InvalidCondaTable)
        })
        .transpose()?;

    if let Some(conda) = conda.as_ref() {
        let unsupported = conda
            .iter()
            .map(|(key, _)| key)
            .filter(|key| !matches!(*key, "channels" | "dependencies"))
            .map(|key| format!("tool.conda.{key}"))
            .collect::<Vec<_>>();
        if !unsupported.is_empty() {
            return Err(ScriptManifestError::UnsupportedFields(unsupported));
        }
    }

    if conda
        .as_ref()
        .is_some_and(|conda| conda.contains_key("channels"))
    {
        let workspace = get_or_insert_table(pixi, "workspace", "tool.pixi.workspace")?;
        if workspace.contains_key("channels") {
            return Err(ScriptManifestError::ConflictingChannels);
        }
        let channels = conda
            .as_mut()
            .and_then(|conda| conda.remove("channels"))
            .expect("channels was checked above");
        if channels.as_array().is_none() {
            return Err(ScriptManifestError::InvalidCondaChannels);
        }
        workspace.insert("channels", channels);
    }

    let mut explicit_names = pixi
        .get("dependencies")
        .and_then(Item::as_table_like)
        .map(conda_dependency_names)
        .transpose()?
        .unwrap_or_default();
    let has_explicit_python = explicit_names.contains("python");

    let Some(conda_dependencies) = conda
        .as_mut()
        .and_then(|conda| conda.remove("dependencies"))
    else {
        validate_pypi_locations(pixi, portable_pypi_names)?;
        return Ok(has_explicit_python);
    };
    let dependencies = conda_dependencies
        .as_array()
        .ok_or(ScriptManifestError::InvalidCondaDependencies)?;
    let parsed = dependencies
        .iter()
        .map(|dependency| {
            let dependency = dependency
                .as_str()
                .ok_or(ScriptManifestError::InvalidCondaDependencies)?;
            let (name, spec) =
                MatchSpec::from_str(dependency, ParseStrictness::Strict)?.into_nameless();
            let name = name
                .as_exact()
                .ok_or_else(|| ScriptManifestError::MissingCondaPackageName(dependency.to_owned()))?
                .clone();
            Ok((name, spec))
        })
        .collect::<Result<Vec<_>, ScriptManifestError>>()?;

    let dependencies = get_or_insert_table(pixi, "dependencies", "tool.pixi.dependencies")?;
    let channel_config = ChannelConfig::default_with_root_dir(root_directory.to_owned());
    for (name, spec) in parsed {
        if !explicit_names.insert(name.as_normalized().to_owned()) {
            return Err(ScriptManifestError::DuplicateCondaDependency(
                name.as_source().to_owned(),
            ));
        }
        dependencies.insert(
            name.as_source(),
            Item::Value(PixiSpec::from_nameless_matchspec(spec, &channel_config).to_toml_value()),
        );
    }

    let has_explicit_python = explicit_names.contains("python");
    validate_pypi_locations(pixi, portable_pypi_names)?;
    Ok(has_explicit_python)
}

fn get_or_insert_table<'a>(
    parent: &'a mut Table,
    key: &str,
    path: &'static str,
) -> Result<&'a mut Table, ScriptManifestError> {
    if !parent.contains_key(key) {
        parent.insert(key, Item::Table(Table::new()));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or(ScriptManifestError::ExpectedTable(path))
}

fn conda_dependency_names(
    dependencies: &dyn toml_edit::TableLike,
) -> Result<HashSet<String>, ScriptManifestError> {
    dependencies
        .iter()
        .map(|(name, _)| Ok(PackageName::from_str(name)?.as_normalized().to_owned()))
        .collect()
}

fn portable_pypi_dependency_names(
    dependencies: &Item,
    root_directory: &Path,
) -> Result<HashSet<String>, ScriptManifestError> {
    let dependencies = dependencies
        .as_array()
        .ok_or(ScriptManifestError::InvalidPypiDependencies)?;
    dependencies
        .iter()
        .map(|dependency| {
            let dependency = dependency
                .as_str()
                .ok_or(ScriptManifestError::InvalidPypiDependencies)?;
            let requirement = pep508_rs::Requirement::parse(dependency, root_directory)?;
            Ok(requirement.name.to_string())
        })
        .collect()
}

fn validate_pypi_locations(
    pixi: &Table,
    portable_names: &HashSet<String>,
) -> Result<(), ScriptManifestError> {
    let Some(rich) = pixi.get("pypi-dependencies").and_then(Item::as_table_like) else {
        return Ok(());
    };
    for (name, _) in rich.iter() {
        let name = PypiPackageName::from_str(name)?;
        if portable_names.contains(name.as_normalized().as_ref()) {
            return Err(ScriptManifestError::DuplicatePypiDependency(
                name.as_source().to_owned(),
            ));
        }
    }
    Ok(())
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

    let Some(pixi) = nested_table(metadata, &["tool", "pixi"]) else {
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

/// Navigate a chain of nested tables, returning `None` when any level is
/// missing or not table-like.
fn nested_table<'a>(
    document: &'a DocumentMut,
    path: &[&str],
) -> Option<&'a dyn toml_edit::TableLike> {
    let mut current: &dyn toml_edit::TableLike = document.as_table();
    for key in path {
        current = current.get(key)?.as_table_like()?;
    }
    Some(current)
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

    #[error("`tool.conda` must be a table")]
    InvalidCondaTable,

    #[error("`tool.conda.channels` must be an array")]
    InvalidCondaChannels,

    #[error("`tool.conda.dependencies` must be an array of MatchSpec strings")]
    InvalidCondaDependencies,

    #[error("`dependencies` must be an array of PEP 508 requirement strings")]
    InvalidPypiDependencies,

    #[error("`tool.conda.channels` and `tool.pixi.workspace.channels` cannot both be set")]
    ConflictingChannels,

    #[error("conda dependency `{0}` does not name exactly one package")]
    MissingCondaPackageName(String),

    #[error(
        "conda dependency `{0}` is declared in both `tool.conda.dependencies` and `tool.pixi.dependencies`"
    )]
    DuplicateCondaDependency(String),

    #[error(
        "PyPI dependency `{0}` is declared in both `dependencies` and `tool.pixi.pypi-dependencies`"
    )]
    DuplicatePypiDependency(String),

    #[error("`{0}` must be a table")]
    ExpectedTable(&'static str),

    #[error("`tool` must be a table")]
    InvalidToolTable,

    #[error("the editable script document is missing its project table")]
    InvalidEditableDocument,

    #[error(transparent)]
    SpecConversion(#[from] pixi_spec::SpecConversionError),

    #[error(transparent)]
    MatchSpec(#[from] rattler_conda_types::ParseMatchSpecError),

    #[error(transparent)]
    VersionSpec(#[from] rattler_conda_types::version_spec::ParseVersionSpecError),

    #[error(transparent)]
    CondaPackageName(#[from] rattler_conda_types::InvalidPackageNameError),

    #[error(transparent)]
    PypiPackageName(#[from] pep508_rs::InvalidNameError),

    #[error(transparent)]
    Pep508(#[from] pep508_rs::Pep508Error),

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
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = []
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
# [tool.conda]
# channels = []
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
# channels = ["conda-forge"]
# dependencies = ["python 3.12.*"]
#
# [tool.pixi.workspace]
# platforms = ["linux-64"]
#
# [tool.pixi.dependencies]
# zlib = "*"
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
        let mut python_specs = target
            .run_dependencies()
            .unwrap()
            .get(&python)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        python_specs.sort();
        assert_eq!(python_specs, ["3.12.*", ">=3.11"]);
        assert!(target.has_dependency(
            &PackageName::from_str("zlib").unwrap(),
            SpecType::Run,
            None
        ));
        assert!(
            target
                .pypi_dependencies
                .as_ref()
                .unwrap()
                .contains_key(&PypiPackageName::from_str("requests").unwrap())
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
    fn rejects_conflicting_conda_and_pixi_channels() {
        let (_directory, path) = script(
            r#"# /// script
# dependencies = []
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["bioconda"]
# ///
"#,
        );

        assert!(matches!(
            ScriptManifest::from_path(path)
                .unwrap()
                .unwrap()
                .into_workspace_manifest(),
            Err(ScriptManifestError::ConflictingChannels)
        ));
    }

    #[test]
    fn rejects_dependencies_declared_in_both_portable_and_rich_locations() {
        let (_directory, conda_path) = script(
            r#"# /// script
# dependencies = []
#
# [tool.conda]
# dependencies = ["NumPy >=2"]
#
# [tool.pixi.dependencies]
# numpy = { version = ">=2", channel = "conda-forge" }
# ///
"#,
        );
        assert!(matches!(
            ScriptManifest::from_path(conda_path)
                .unwrap()
                .unwrap()
                .into_workspace_manifest(),
            Err(ScriptManifestError::DuplicateCondaDependency(name)) if name == "NumPy"
        ));

        let (_directory, pypi_path) = script(
            r#"# /// script
# dependencies = ["typing_extensions>=4"]
#
# [tool.pixi.pypi-dependencies]
# typing-extensions = { version = ">=4", index = "https://pypi.org/simple" }
# ///
"#,
        );
        assert!(matches!(
            ScriptManifest::from_path(pypi_path)
                .unwrap()
                .unwrap()
                .into_workspace_manifest(),
            Err(ScriptManifestError::DuplicatePypiDependency(name)) if name == "typing-extensions"
        ));
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
# ///

print("hello")
"#
        );
    }

    #[test]
    fn pyproject_edits_keep_portable_and_rich_dependencies_separate() {
        let (_directory, path) = script(
            r#"# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["python >=3.11"]
#
# [tool.pixi.dependencies]
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
        let portable = metadata["tool"]["conda"]["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| {
                MatchSpec::from_str(value.as_str().unwrap(), ParseStrictness::Strict)
                    .unwrap()
                    .name
                    .as_exact()
                    .unwrap()
                    .as_normalized()
                    .to_owned()
            })
            .collect::<HashSet<_>>();
        assert_eq!(portable, HashSet::from(["numpy".into(), "python".into()]));
        let rich = metadata["tool"]["pixi"]["dependencies"].as_table().unwrap();
        assert!(rich.contains_key("openssl"));
        assert!(!rich.contains_key("numpy"));
    }

    #[test]
    fn pyproject_edits_do_not_persist_solver_only_python() {
        let (_directory, path) = script(
            r#"# /// script
# dependencies = ["requests>=2"]
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = []
# ///
"#,
        );
        let script = ScriptManifest::from_path(&path).unwrap().unwrap();
        let mut pyproject = script.pyproject_document().unwrap();
        pyproject["project"]["dependencies"]
            .as_array_mut()
            .unwrap()
            .clear();

        let rendered = script.render_pyproject_document(&pyproject).unwrap();
        fs_err::write(&path, rendered).unwrap();
        let metadata = ScriptManifest::from_path(path)
            .unwrap()
            .unwrap()
            .metadata_document()
            .unwrap();

        assert!(
            metadata["tool"]["conda"]["dependencies"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
