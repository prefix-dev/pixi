//! This module provides [`toml_span`] parsing functionality for
//! `pyproject.toml` files.

use std::str::FromStr;

use pep508_rs::Requirement;
use pixi_toml::{DeserializeAs, Same, TomlFromStr, TomlIndexMap, TomlWith};
use pyproject_toml::{
    BuildSystem, Contact, DependencyGroupSpecifier, DependencyGroups, License, Project,
    PyProjectToml, ReadMe,
};
use toml_span::{
    de_helpers::{expected, TableHelper},
    value::ValueInner,
    DeserError, Deserialize, Error, ErrorKind, Value,
};

use crate::pyproject::{PyProjectManifest, Tool, ToolPoetry};

impl<'de> toml_span::Deserialize<'de> for PyProjectManifest {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let build_system = th.optional("build-system").map(TomlBuildSystem::into_inner);
        let project = th.optional("project").map(TomlProject::into_inner);
        let dependency_groups = th
            .optional("dependency-groups")
            .map(TomlDependencyGroups::into_inner);
        let tool = th.optional("tool");

        th.finalize(None)?;

        Ok(PyProjectManifest {
            inner: PyProjectToml {
                build_system,
                project,
                dependency_groups,
            },
            tool,
        })
    }
}

/// A wrapper around [`BuildSystem`] that implements [`toml_span::Deserialize`]
/// and [`pixi_toml::DeserializeAs`].
struct TomlBuildSystem(BuildSystem);

impl TomlBuildSystem {
    pub fn into_inner(self) -> BuildSystem {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlBuildSystem {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let requires = th
            .required::<TomlWith<_, Vec<TomlFromStr<_>>>>("requires")?
            .into_inner();
        let build_backend = th.optional("build-backend");
        let backend_path = th.optional("backend-path");
        th.finalize(Some(value))?;
        Ok(Self(BuildSystem {
            requires,
            build_backend,
            backend_path,
        }))
    }
}

impl<'de> DeserializeAs<'de, BuildSystem> for TomlBuildSystem {
    fn deserialize_as(value: &mut Value<'de>) -> Result<BuildSystem, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
    }
}

/// A wrapper around [`Project`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
struct TomlProject(Project);

impl TomlProject {
    pub fn into_inner(self) -> Project {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlProject {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let name = th.required("name")?;
        let version = th
            .optional::<TomlFromStr<_>>("version")
            .map(TomlFromStr::into_inner);
        let description = th.optional("description");
        let readme = th.optional("readme").map(TomlReadme::into_inner);
        let requires_python = th
            .optional::<TomlFromStr<_>>("requires-python")
            .map(TomlFromStr::into_inner);
        let license = th.optional("license").map(TomlLicense::into_inner);
        let license_files = th.optional("license-files");
        let authors = th
            .optional::<TomlWith<_, Vec<TomlContact>>>("authors")
            .map(TomlWith::into_inner);
        let maintainers = th
            .optional::<TomlWith<_, Vec<TomlContact>>>("maintainers")
            .map(TomlWith::into_inner);
        let keywords = th.optional("keywords");
        let classifiers = th.optional("classifiers");
        let urls = th
            .optional::<TomlIndexMap<_, _>>("urls")
            .map(TomlIndexMap::into_inner);
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
        let dependencies = th
            .optional::<TomlWith<_, Vec<TomlFromStr<_>>>>("dependencies")
            .map(TomlWith::into_inner);
        let optional_dependencies = th
            .optional::<TomlWith<_, TomlIndexMap<_, Vec<TomlFromStr<_>>>>>("optional-dependencies")
            .map(TomlWith::into_inner);
        let dynamic = th.optional("dynamic");

        th.finalize(None)?;

        Ok(Self(Project {
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
        }))
    }
}

impl<'de> DeserializeAs<'de, Project> for TomlProject {
    fn deserialize_as(value: &mut Value<'de>) -> Result<Project, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
    }
}

/// A wrapper around [`ReadMe`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
struct TomlReadme(ReadMe);

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
struct TomlLicense(License);

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
struct TomlContact(Contact);

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

/// A wrapper around [`DependencyGroups`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
struct TomlDependencyGroups(DependencyGroups);

impl TomlDependencyGroups {
    pub fn into_inner(self) -> DependencyGroups {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlDependencyGroups {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        Ok(Self(DependencyGroups(
            TomlWith::<_, TomlIndexMap<_, Vec<TomlDependencyGroupSpecifier>>>::deserialize(value)?
                .into_inner(),
        )))
    }
}

impl<'de> DeserializeAs<'de, DependencyGroups> for TomlDependencyGroups {
    fn deserialize_as(value: &mut Value<'de>) -> Result<DependencyGroups, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
    }
}

/// A wrapper around [`DependencyGroupSpecifier`] that implements [`toml_span::Deserialize`] and
/// [`pixi_toml::DeserializeAs`].
struct TomlDependencyGroupSpecifier(DependencyGroupSpecifier);

impl TomlDependencyGroupSpecifier {
    pub fn into_inner(self) -> DependencyGroupSpecifier {
        self.0
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlDependencyGroupSpecifier {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(str) => Ok(Self(DependencyGroupSpecifier::String(
                Requirement::from_str(&str).map_err(|e| {
                    DeserError::from(Error {
                        kind: ErrorKind::Custom(e.message.to_string().into()),
                        span: value.span,
                        line_info: None,
                    })
                })?,
            ))),
            ValueInner::Table(table) => {
                let mut th = TableHelper::from((table, value.span));
                let include_group = th.required("include-group")?;
                th.finalize(None)?;
                Ok(Self(DependencyGroupSpecifier::Table { include_group }))
            }
            inner => Err(DeserError::from(expected(
                "a string or table",
                inner,
                value.span,
            ))),
        }
    }
}

impl<'de> DeserializeAs<'de, DependencyGroupSpecifier> for TomlDependencyGroupSpecifier {
    fn deserialize_as(value: &mut Value<'de>) -> Result<DependencyGroupSpecifier, DeserError> {
        Self::deserialize(value).map(Self::into_inner)
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
