//! This module provides [`toml_span`] parsing functionality for
//! `pyproject.toml` files.

use std::path::Path;

use indexmap::IndexMap;
use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::Requirement;
use pixi_toml::{DeserializeAs, Same, TomlFromStr, TomlIndexMap, TomlWith};
use pyproject_toml::{
    self, BuildSystem, Contact, DependencyGroupSpecifier, DependencyGroups, License, Project,
    ReadMe,
};
use toml_span::{
    DeserError, Deserialize, Error, ErrorKind, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::error::{GenericError, TomlError};
use crate::pyproject::{PyProjectManifest, Tool, ToolPoetry};

#[derive(Debug)]
pub struct PyProjectToml {
    pub project: Option<TomlProject>,
    pub build_system: Option<TomlBuildSystem>,
    pub dependency_groups: Option<Spanned<TomlDependencyGroups>>,
}

impl PyProjectToml {
    pub fn into_inner(
        self,
        working_dir: &Path,
    ) -> Result<pyproject_toml::PyProjectToml, TomlError> {
        Ok(pyproject_toml::PyProjectToml {
            project: self
                .project
                .map(|p| p.into_inner(working_dir))
                .transpose()?,
            build_system: self.build_system.map(TomlBuildSystem::into_inner),
            dependency_groups: self
                .dependency_groups
                .map(Spanned::take)
                .map(|dg| dg.into_inner(working_dir))
                .transpose()?,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for PyProjectToml {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let build_system = th.optional("build-system");
        let project = th.optional("project");
        let dependency_groups = th.optional("dependency-groups");

        th.finalize(Some(value))?;
        Ok(PyProjectToml {
            project,
            build_system,
            dependency_groups,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for PyProjectManifest {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let project = PyProjectToml::deserialize(value)?;

        let mut th = TableHelper::new(value)?;
        let tools = th.optional("tool");
        th.finalize(Some(value))?;

        Ok(PyProjectManifest {
            project,
            tool: tools,
        })
    }
}

/// A wrapper around [`BuildSystem`] that implements [`toml_span::Deserialize`]
/// and [`pixi_toml::DeserializeAs`].
#[derive(Debug)]
pub struct TomlBuildSystem {
    /// PEP 508 dependencies required to execute the build system
    pub requires: Vec<Spanned<Requirement>>,
    /// A string naming a Python object that will be used to perform the build
    pub build_backend: Option<Spanned<String>>,
    /// Specify that their backend code is hosted in-tree, this key contains a
    /// list of directories
    pub backend_path: Option<Vec<Spanned<String>>>,
}

impl TomlBuildSystem {
    pub fn into_inner(self) -> BuildSystem {
        BuildSystem {
            requires: self.requires.into_iter().map(Spanned::take).collect(),
            build_backend: self.build_backend.map(Spanned::take),
            backend_path: self
                .backend_path
                .map(|backend_path| backend_path.into_iter().map(Spanned::take).collect()),
        }
    }
}

impl<'de> Deserialize<'de> for TomlBuildSystem {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let requires = th
            .required::<TomlWith<_, Vec<Spanned<TomlFromStr<_>>>>>("requires")?
            .into_inner();
        let build_backend = th.optional("build-backend");
        let backend_path = th.optional("backend-path");
        th.finalize(Some(value))?;
        Ok(Self {
            requires,
            build_backend,
            backend_path,
        })
    }
}

impl<'de> DeserializeAs<'de, BuildSystem> for TomlBuildSystem {
    fn deserialize_as(value: &mut Value<'de>) -> Result<BuildSystem, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
    }
}

#[derive(Debug)]
pub struct TomlProject {
    /// The name of the project
    pub name: Spanned<String>,
    /// The version of the project as supported by PEP 440
    pub version: Option<Spanned<Version>>,
    /// The summary description of the project
    pub description: Option<Spanned<String>>,
    /// The full description of the project (i.e. the README)
    pub readme: Option<Spanned<TomlReadme>>,
    /// The Python version requirements of the project
    pub requires_python: Option<Spanned<VersionSpecifiers>>,
    /// The license under which the project is distributed
    ///
    /// Supports both the current standard and the provisional PEP 639
    pub license: Option<Spanned<TomlLicense>>,
    /// The paths to files containing licenses and other legal notices to be
    /// distributed with the project.
    ///
    /// Use `parse_pep639_glob` from the optional `pep639-glob` feature to find
    /// the matching files.
    ///
    /// Note that this doesn't check the PEP 639 rules for combining
    /// `license_files` and `license`.
    ///
    /// From the provisional PEP 639
    pub license_files: Option<Vec<Spanned<String>>>,
    /// The people or organizations considered to be the "authors" of the
    /// project
    pub authors: Option<Vec<Spanned<TomlContact>>>,
    /// Similar to "authors" in that its exact meaning is open to interpretation
    pub maintainers: Option<Vec<Spanned<TomlContact>>>,
    /// The keywords for the project
    pub keywords: Option<Vec<Spanned<String>>>,
    /// Trove classifiers which apply to the project
    pub classifiers: Option<Vec<Spanned<String>>>,
    /// A table of URLs where the key is the URL label and the value is the URL
    /// itself
    pub urls: Option<IndexMap<String, Spanned<String>>>,
    /// Entry points
    pub entry_points: Option<IndexMap<String, IndexMap<String, Spanned<String>>>>,
    /// Corresponds to the console_scripts group in the core metadata
    pub scripts: Option<IndexMap<String, Spanned<String>>>,
    /// Corresponds to the gui_scripts group in the core metadata
    pub gui_scripts: Option<IndexMap<String, Spanned<String>>>,
    /// Project dependencies (stored as raw strings, parsed later with working_dir)
    pub dependencies: Option<Vec<Spanned<String>>>,
    /// Optional dependencies (stored as raw strings, parsed later with working_dir)
    pub optional_dependencies: Option<IndexMap<String, Vec<Spanned<String>>>>,
    /// Specifies which fields listed by PEP 621 were intentionally unspecified
    /// so another tool can/will provide such metadata dynamically.
    pub dynamic: Option<Vec<Spanned<String>>>,
}

impl TomlProject {
    pub fn into_inner(self, working_dir: &Path) -> Result<Project, TomlError> {
        let dependencies = self
            .dependencies
            .map(|deps| {
                deps.into_iter()
                    .map(|s| parse_requirement_with_dir(&s, working_dir))
                    .inspect(|v| eprintln!("Debug VersionOrUrl: {v:?}"))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let optional_dependencies = self
            .optional_dependencies
            .map(|opt_deps| {
                opt_deps
                    .into_iter()
                    .map(|(key, deps)| {
                        let parsed = deps
                            .into_iter()
                            .map(|s| parse_requirement_with_dir(&s, working_dir))
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok((key, parsed))
                    })
                    .collect::<Result<IndexMap<_, _>, TomlError>>()
            })
            .transpose()?;

        Ok(Project {
            name: self.name.take(),
            version: self.version.map(Spanned::take),
            description: self.description.map(Spanned::take),
            readme: self.readme.map(Spanned::take).map(TomlReadme::into_inner),
            requires_python: self.requires_python.map(Spanned::take),
            license: self.license.map(Spanned::take).map(TomlLicense::into_inner),
            license_files: self
                .license_files
                .map(|files| files.into_iter().map(Spanned::take).collect()),
            authors: self.authors.map(|authors| {
                authors
                    .into_iter()
                    .map(Spanned::take)
                    .map(TomlContact::into_inner)
                    .collect()
            }),
            maintainers: self.maintainers.map(|maintainers| {
                maintainers
                    .into_iter()
                    .map(Spanned::take)
                    .map(TomlContact::into_inner)
                    .collect()
            }),
            keywords: self
                .keywords
                .map(|keywords| keywords.into_iter().map(Spanned::take).collect()),
            classifiers: self
                .classifiers
                .map(|classifiers| classifiers.into_iter().map(Spanned::take).collect()),
            urls: self
                .urls
                .map(|urls| urls.into_iter().map(|(k, v)| (k, v.take())).collect()),
            entry_points: self.entry_points.map(|entry_points| {
                entry_points
                    .into_iter()
                    .map(|(k, v)| (k, v.into_iter().map(|(k, v)| (k, v.take())).collect()))
                    .collect()
            }),
            scripts: self
                .scripts
                .map(|scripts| scripts.into_iter().map(|(k, v)| (k, v.take())).collect()),
            gui_scripts: self.gui_scripts.map(|gui_scripts| {
                gui_scripts
                    .into_iter()
                    .map(|(k, v)| (k, v.take()))
                    .collect()
            }),
            dependencies,
            optional_dependencies,
            dynamic: self
                .dynamic
                .map(|dynamic| dynamic.into_iter().map(Spanned::take).collect()),
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlProject {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let name = th.required("name")?;
        let version = th
            .optional::<TomlWith<_, Spanned<TomlFromStr<_>>>>("version")
            .map(TomlWith::into_inner);
        let description = th.optional("description");
        let readme = th.optional("readme");
        let requires_python = th
            .optional::<TomlWith<_, Spanned<TomlFromStr<_>>>>("requires-python")
            .map(TomlWith::into_inner);
        let license = th.optional("license");
        let license_files = th.optional("license-files");
        let authors = th.optional("authors");
        let maintainers = th.optional("maintainers");
        let keywords = th.optional("keywords");
        let classifiers = th.optional("classifiers");
        let urls = th
            .optional::<TomlWith<_, TomlIndexMap<_, Spanned<Same>>>>("urls")
            .map(TomlWith::into_inner);
        let entry_points = th
            .optional::<TomlWith<_, TomlIndexMap<String, TomlIndexMap<String, Same>>>>(
                "entry-points",
            )
            .map(TomlWith::into_inner);
        let scripts = th
            .optional::<TomlIndexMap<_, _>>("scripts")
            .map(TomlIndexMap::into_inner);
        let gui_scripts = th
            .optional::<TomlIndexMap<_, _>>("gui-scripts")
            .map(TomlIndexMap::into_inner);
        let dependencies: Option<Vec<Spanned<String>>> = th.optional("dependencies");
        let optional_dependencies = th
            .optional::<TomlWith<_, TomlIndexMap<_, Vec<Spanned<Same>>>>>("optional-dependencies")
            .map(TomlWith::into_inner);
        let dynamic = th.optional("dynamic");

        th.finalize(None)?;

        Ok(Self {
            name,
            version,
            description,
            readme,
            requires_python,
            license,
            license_files,
            authors,
            maintainers,
            keywords,
            classifiers,
            urls,
            entry_points,
            scripts,
            gui_scripts,
            dependencies,
            optional_dependencies,
            dynamic,
        })
    }
}

/// Parse a raw PEP 508 string into a [`Requirement`], optionally using a
/// working directory to resolve relative paths.
fn parse_requirement_with_dir(
    spanned: &Spanned<String>,
    working_dir: &Path,
) -> Result<Requirement, TomlError> {
    Requirement::parse(&spanned.value, working_dir).map_err(|e| {
        GenericError::new(e.message.to_string())
            .with_span(spanned.span.start..spanned.span.end)
            .into()
    })
}

/// A wrapper around [`ReadMe`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
#[derive(Debug)]
pub struct TomlReadme(ReadMe);

impl TomlReadme {
    pub fn into_inner(self) -> ReadMe {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlReadme {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => Ok(Self(ReadMe::RelativePath(str.into_owned()))),
            ValueInner::Table(table) => {
                let mut th = TableHelper::from((table, value.span));
                let file = th.optional("file");
                let text = th.optional("text");
                let content_type = th.optional("content-type");
                th.finalize(None)?;
                Ok(Self(ReadMe::Table {
                    file,
                    text,
                    content_type,
                }))
            }
            inner => Err(expected("a string or table", inner, value.span).into()),
        }
    }
}

impl<'de> DeserializeAs<'de, ReadMe> for TomlReadme {
    fn deserialize_as(value: &mut Value<'de>) -> Result<ReadMe, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
    }
}

/// A wrapper around [`License`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
#[derive(Debug)]
pub struct TomlLicense(License);

impl TomlLicense {
    pub fn into_inner(self) -> License {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlLicense {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => Ok(Self(License::Spdx(str.into_owned()))),
            ValueInner::Table(table) => {
                let mut th = TableHelper::from((table, value.span));
                if th.contains("text") {
                    let text = th.required("text")?;
                    th.finalize(None)?;
                    Ok(Self(License::Text { text }))
                } else if th.contains("file") {
                    let file = th.required::<String>("file")?.into();
                    th.finalize(None)?;
                    Ok(Self(License::File { file }))
                } else {
                    Err(DeserError::from(Error {
                        kind: ErrorKind::UnexpectedKeys {
                            keys: th
                                .table
                                .into_keys()
                                .map(|k| (k.name.into_owned(), k.span))
                                .collect(),
                            expected: vec!["text".into(), "file".into()],
                        },
                        span: value.span,
                        line_info: None,
                    }))
                }
            }
            inner => Err(expected("a string or table", inner, value.span).into()),
        }
    }
}

impl<'de> DeserializeAs<'de, License> for TomlLicense {
    fn deserialize_as(value: &mut Value<'de>) -> Result<License, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
    }
}

/// A wrapper around [`Contact`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
#[derive(Debug)]
pub struct TomlContact(Contact);

impl TomlContact {
    pub fn into_inner(self) -> Contact {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlContact {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let name = th.optional("name");
        let email = th.optional("email");

        th.finalize(None)?;

        match (name, email) {
            (Some(name), Some(email)) => Ok(Self(Contact::NameEmail { name, email })),
            (None, Some(email)) => Ok(Self(Contact::Email { email })),
            (Some(name), None) => Ok(Self(Contact::Name { name })),
            (None, None) => Err(DeserError::from(Error {
                kind: ErrorKind::MissingField("name"),
                span: value.span,
                line_info: None,
            })),
        }
    }
}

impl<'de> DeserializeAs<'de, Contact> for TomlContact {
    fn deserialize_as(value: &mut Value<'de>) -> Result<Contact, DeserError> {
        TomlContact::deserialize(value).map(TomlContact::into_inner)
    }
}

/// Intermediate representation of `[dependency-groups]` that stores requirement
/// strings unparsed. The strings are resolved into [`Requirement`] objects in
/// [`TomlDependencyGroups::into_inner`] where a working directory can be
/// provided.
#[derive(Debug)]
pub struct TomlDependencyGroups(pub IndexMap<String, Vec<TomlDependencyGroupSpecifier>>);

impl TomlDependencyGroups {
    pub fn into_inner(self, working_dir: &Path) -> Result<DependencyGroups, TomlError> {
        let mut groups = IndexMap::new();
        for (name, specifiers) in self.0 {
            let parsed = specifiers
                .into_iter()
                .map(|spec| match spec {
                    TomlDependencyGroupSpecifier::String(spanned) => {
                        let req = parse_requirement_with_dir(&spanned, working_dir)?;
                        Ok(DependencyGroupSpecifier::String(req))
                    }
                    TomlDependencyGroupSpecifier::Table { include_group } => {
                        Ok(DependencyGroupSpecifier::Table { include_group })
                    }
                })
                .collect::<Result<Vec<_>, TomlError>>()?;
            groups.insert(name, parsed);
        }
        Ok(DependencyGroups(groups))
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlDependencyGroups {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let map = TomlIndexMap::<String, Vec<TomlDependencyGroupSpecifier>>::deserialize(value)?;
        Ok(Self(map.into_inner()))
    }
}

/// Intermediate representation of a dependency group specifier that stores
/// requirement strings unparsed.
#[derive(Debug)]
pub enum TomlDependencyGroupSpecifier {
    /// Raw PEP 508 string, parsed later with working_dir context
    String(Spanned<String>),
    /// Include another dependency group
    Table { include_group: String },
}

impl<'de> toml_span::Deserialize<'de> for TomlDependencyGroupSpecifier {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        match value.take() {
            ValueInner::String(str) => Ok(TomlDependencyGroupSpecifier::String(
                Spanned::with_span(str.into_owned(), span),
            )),
            ValueInner::Table(table) => {
                let mut th = TableHelper::from((table, value.span));
                let include_group = th.required("include-group")?;
                th.finalize(None)?;
                Ok(TomlDependencyGroupSpecifier::Table { include_group })
            }
            inner => Err(DeserError::from(expected(
                "a string or table",
                inner,
                value.span,
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for ToolPoetry {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let name = th.optional("name");
        let description = th.optional("description");
        let version = th.optional("version");
        let authors = th.optional("authors");

        Ok(Self {
            name,
            description,
            version,
            authors,
        })
    }
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let poetry = th.optional("poetry");
        let pixi = th.optional("pixi");

        th.finalize(Some(value))?;

        Ok(Self { poetry, pixi })
    }
}
