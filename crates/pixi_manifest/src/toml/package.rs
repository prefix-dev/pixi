use std::path::{Path, PathBuf};

use indexmap::IndexMap;
pub use pixi_toml::TomlFromStr;
use pixi_toml::{DeserializeAs, Same, TomlIndexMap, TomlWith};
use rattler_conda_types::Version;
use thiserror::Error;
use toml_span::{
    de_helpers::TableHelper, DeserError, Error, ErrorKind, Span, Spanned, Value,
};
use url::Url;

use crate::toml::manifest::ExternalWorkspaceProperties;
use crate::{
    error::GenericError,
    package::Package,
    toml::{package_target::TomlPackageTarget, TomlPackageBuild},
    utils::{package_map::UniquePackageMap, PixiSpanned},
    PackageManifest, Preview, TargetSelector, Targets, TomlError, WithWarnings,
};

/// Represents a field that can either have a direct value or inherit from workspace
#[derive(Debug, Clone)]
pub enum WorkspaceInheritableField<T> {
    /// Direct value specified in the package
    Value(T),
    /// Inherit the value from the workspace
    Workspace(Span),
}

impl<T> WorkspaceInheritableField<T> {
    /// Get the value if it's a direct value, otherwise return None
    pub fn value(self) -> Option<T> {
        match self {
            WorkspaceInheritableField::Value(v) => Some(v),
            WorkspaceInheritableField::Workspace(_) => None,
        }
    }

    /// Check if this field should inherit from workspace
    pub fn is_workspace(&self) -> bool {
        matches!(self, WorkspaceInheritableField::Workspace(_))
    }

    /// Map the inner value if it's a Value variant
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> WorkspaceInheritableField<U> {
        match self {
            WorkspaceInheritableField::Value(v) => WorkspaceInheritableField::Value(f(v)),
            WorkspaceInheritableField::Workspace(span) => WorkspaceInheritableField::Workspace(span),
        }
    }

    /// Resolve the field value, either using the direct value or inheriting from workspace
    pub fn resolve(self, workspace_value: Option<T>) -> Option<T> {
        match self {
            WorkspaceInheritableField::Value(v) => Some(v),
            WorkspaceInheritableField::Workspace(_) => workspace_value,
        }
    }
}

impl<'de, T> toml_span::Deserialize<'de> for WorkspaceInheritableField<T>
where
    T: toml_span::Deserialize<'de>,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        // First check if it's a table without consuming the value, then take it if it is
        if value.as_table().is_some() {
            let mut th = TableHelper::new(value)?;
            let workspace = th.optional::<bool>("workspace");
            th.finalize(None)?;

            if let Some(true) = workspace {
                return Ok(WorkspaceInheritableField::Workspace(value.span));
            } else if let Some(false) = workspace {
                return Err(DeserError::from(Error {
                    kind: ErrorKind::Custom("workspace inheritance must be `true`".into()),
                    span: value.span,
                    line_info: None,
                }));
            }
        }

        // If not a table or not { workspace = true }, try to deserialize as direct value
        T::deserialize(value).map(WorkspaceInheritableField::Value)
    }
}

impl<'de, T, U> DeserializeAs<'de, WorkspaceInheritableField<T>> for WorkspaceInheritableField<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<WorkspaceInheritableField<T>, DeserError> {
        // First check if it's a table without consuming the value, then take it if it is
        if value.as_table().is_some() {
            let mut th = TableHelper::new(value)?;
            let workspace = th.optional::<bool>("workspace");
            th.finalize(None)?;

            if let Some(true) = workspace {
                return Ok(WorkspaceInheritableField::Workspace(value.span));
            } else if let Some(false) = workspace {
                return Err(DeserError::from(Error {
                    kind: ErrorKind::Custom("workspace inheritance must be `true`".into()),
                    span: value.span,
                    line_info: None,
                }));
            }
        }

        // If not a table or not { workspace = true }, try to deserialize as direct value
        U::deserialize_as(value).map(WorkspaceInheritableField::Value)
    }
}

/// The TOML representation of the `[package]` section in a pixi manifest.
///
/// In TOML some of the fields can be empty even though they are required in the
/// data model (e.g. `name`, `version`). This is allowed because some of the
/// fields might be derived from other sections of the TOML.
#[derive(Debug)]
pub struct TomlPackage {
    // Fields that can be inherited from workspace or specified directly
    pub name: Option<WorkspaceInheritableField<String>>,
    pub version: Option<WorkspaceInheritableField<Version>>,
    pub description: Option<WorkspaceInheritableField<String>>,
    pub authors: Option<WorkspaceInheritableField<Vec<String>>>,
    pub license: Option<WorkspaceInheritableField<Spanned<String>>>,
    pub license_file: Option<WorkspaceInheritableField<Spanned<PathBuf>>>,
    pub readme: Option<WorkspaceInheritableField<Spanned<PathBuf>>>,
    pub homepage: Option<WorkspaceInheritableField<Url>>,
    pub repository: Option<WorkspaceInheritableField<Url>>,
    pub documentation: Option<WorkspaceInheritableField<Url>>,

    // Fields that are package-specific and cannot be inherited
    pub build: TomlPackageBuild,
    pub host_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub run_dependencies: Option<PixiSpanned<UniquePackageMap>>,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlPackageTarget>,

    pub span: Span,
}

impl<'de> toml_span::Deserialize<'de> for TomlPackage {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let name = th.optional("name");
        let version = th
            .optional::<TomlWith<_, WorkspaceInheritableField<TomlFromStr<Version>>>>("version")
            .map(TomlWith::into_inner);
        let description = th.optional("description");
        let authors = th.optional("authors");
        let license = th.optional("license");
        let license_file = th
            .optional::<TomlWith<_, WorkspaceInheritableField<Spanned<TomlFromStr<PathBuf>>>>>(
                "license-file",
            )
            .map(TomlWith::into_inner);
        let readme = th
            .optional::<TomlWith<_, WorkspaceInheritableField<Spanned<TomlFromStr<PathBuf>>>>>(
                "readme",
            )
            .map(TomlWith::into_inner);
        let homepage = th
            .optional::<TomlWith<_, WorkspaceInheritableField<TomlFromStr<Url>>>>("homepage")
            .map(TomlWith::into_inner);
        let repository = th
            .optional::<TomlWith<_, WorkspaceInheritableField<TomlFromStr<Url>>>>("repository")
            .map(TomlWith::into_inner);
        let documentation = th
            .optional::<TomlWith<_, WorkspaceInheritableField<TomlFromStr<Url>>>>("documentation")
            .map(TomlWith::into_inner);
        let host_dependencies = th.optional("host-dependencies");
        let build_dependencies = th.optional("build-dependencies");
        let run_dependencies = th.optional("run-dependencies");
        let build = th.required("build")?;
        let target = th
            .optional::<TomlWith<_, TomlIndexMap<_, Same>>>("target")
            .map(TomlWith::into_inner)
            .unwrap_or_default();
        th.finalize(None)?;

        Ok(TomlPackage {
            name,
            version,
            description,
            authors,
            license,
            license_file,
            readme,
            homepage,
            repository,
            documentation,
            host_dependencies,
            build_dependencies,
            run_dependencies,
            build,
            target,
            span: value.span,
        })
    }
}

/// Defines workspace properties that can be inherited by packages.
///
/// This contains the workspace values that packages can inherit from.
#[derive(Debug, Clone, Default)]
pub struct WorkspacePackageProperties {
    pub name: Option<String>,
    pub version: Option<Version>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    // The absolute path to the license file
    pub license_file: Option<PathBuf>,
    // The absolute path to the README
    pub readme: Option<PathBuf>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
}

impl From<ExternalWorkspaceProperties> for WorkspacePackageProperties {
    fn from(value: ExternalWorkspaceProperties) -> Self {
        Self {
            name: value.name,
            version: value.version,
            description: value.description,
            authors: value.authors,
            license: value.license,
            license_file: value.license_file,
            readme: value.readme,
            homepage: value.homepage,
            repository: value.repository,
            documentation: value.documentation,
        }
    }
}

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("missing `name` in `[package]` section")]
    MissingName,

    #[error("missing `version` in `[package]` section")]
    MissingVersion,

    #[error(transparent)]
    TomlError(#[from] TomlError),
}

impl TomlPackage {
    /// Helper function to resolve a required field with proper error messages
    fn resolve_required_field<T>(
        field: Option<WorkspaceInheritableField<T>>,
        workspace_value: Option<T>,
        field_name: &'static str,
        package_span: Span,
    ) -> Result<T, Error> {
        match field {
            Some(WorkspaceInheritableField::Value(v)) => Ok(v),
            Some(WorkspaceInheritableField::Workspace(span)) => {
                workspace_value.ok_or_else(|| Error {
                    kind: ErrorKind::Custom(format!("the workspace does not define a '{}'", field_name).into()),
                    span,
                    line_info: None,
                })
            },
            None => Err(Error {
                kind: ErrorKind::MissingField(field_name),
                span: package_span,
                line_info: None,
            }),
        }
    }

    /// The `root_directory` is used to resolve relative paths, if it is `None`,
    /// paths are not checked.
    pub fn into_manifest(
        self,
        workspace: WorkspacePackageProperties,
        preview: &Preview,
        root_directory: Option<&Path>,
    ) -> Result<WithWarnings<PackageManifest>, TomlError> {
        let warnings = Vec::new();

        // Resolve fields with explicit inheritance
        let name = Self::resolve_required_field(self.name, workspace.name, "name", self.span)?;
        let version = Self::resolve_required_field(self.version, workspace.version, "version", self.span)?;

        let default_package_target = TomlPackageTarget {
            run_dependencies: self.run_dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
        }
        .into_package_target(preview)?;

        let targets = self
            .target
            .into_iter()
            .map(|(selector, target)| {
                let target = target.into_package_target(preview)?;
                Ok::<_, TomlError>((selector, target))
            })
            .collect::<Result<_, _>>()?;

        if let Some(WorkspaceInheritableField::Value(Spanned {
            value: license,
            span,
        })) = &self.license
        {
            if let Err(e) = spdx::Expression::parse(license) {
                return Err(
                    GenericError::new("'license' is not a valid SPDX expression")
                        .with_span((*span).into())
                        .with_span_label(e.to_string())
                        .into(),
                );
            }
        }

        // Check file existence for resolved paths
        fn check_resolved_file(
            root_directory: Option<&Path>,
            field: Option<WorkspaceInheritableField<Spanned<PathBuf>>>,
            workspace_value: Option<PathBuf>,
        ) -> Result<Option<PathBuf>, TomlError> {
            let Some(root_directory) = root_directory else {
                return Ok(None);
            };
            match field {
                None => Ok(None),
                Some(WorkspaceInheritableField::Workspace(_)) => {
                    Ok(workspace_value
                        .and_then(|value| pathdiff::diff_paths(value, root_directory)))
                }
                Some(WorkspaceInheritableField::Value(Spanned { value, span })) => {
                    let full_path = root_directory.join(&value);
                    if !full_path.is_file() {
                        Err(TomlError::from(
                            GenericError::new(format!(
                                "'{}' does not exist",
                                dunce::simplified(&full_path).display()
                            ))
                                .with_span(span.into()),
                        ))
                    } else {
                        Ok(pathdiff::diff_paths(full_path, root_directory))
                    }
                }
            }
        }

        let license_file =
            check_resolved_file(root_directory, self.license_file, workspace.license_file)?;
        let readme = check_resolved_file(root_directory, self.readme, workspace.readme)?;

        Ok(WithWarnings::from(PackageManifest {
            package: Package {
                name,
                version,
                description: self
                    .description
                    .and_then(|field| field.resolve(workspace.description)),
                authors: self
                    .authors
                    .and_then(|field| field.resolve(workspace.authors)),
                license: self
                    .license
                    .and_then(|field| field.map(Spanned::take).resolve(workspace.license)),
                license_file,
                readme,
                homepage: self
                    .homepage
                    .and_then(|field| field.resolve(workspace.homepage)),
                repository: self
                    .repository
                    .and_then(|field| field.resolve(workspace.repository)),
                documentation: self
                    .documentation
                    .and_then(|field| field.resolve(workspace.documentation)),
            },
            build: self.build.into_build_system()?,
            targets: Targets::from_default_and_user_defined(default_package_target, targets),
        })
        .with_warnings(warnings))
    }
}

#[cfg(test)]
mod test {
    use insta::assert_snapshot;

    use super::*;
    use crate::{toml::FromTomlStr, utils::test_utils::format_parse_error};

    #[must_use]
    fn expect_parse_failure(pixi_toml: &str) -> String {
        let parse_error = TomlPackage::from_toml_str(pixi_toml).unwrap_err();
        format_parse_error(pixi_toml, parse_error)
    }

    #[test]
    fn test_invalid_version() {
        assert_snapshot!(expect_parse_failure(
            r#"
        version = "a!0"

        [build]
        backend = { name = "bla", version = "1.0" }"#
        ));
    }

    #[test]
    fn test_invalid_extra_key() {
        assert_snapshot!(expect_parse_failure(
            r#"
        foo = "bar"
        name = "bla"
        extra = "key"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#,
        ));
    }

    #[test]
    fn test_invalid_license_file() {
        let input = r#"
        name = "bla"
        version = "1.0"
        license-file = "LICENSE.txt"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        let path = Path::new("");
        let parse_error = TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    &Preview::default(),
                    Some(path),
                )
            })
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error),@r###"
         × 'LICENSE.txt' does not exist
          ╭─[pixi.toml:4:25]
        3 │         version = "1.0"
        4 │         license-file = "LICENSE.txt"
          ·                         ───────────
        5 │
          ╰────
        "###);
    }

    #[test]
    fn test_invalid_readme() {
        let input = r#"
        name = "bla"
        version = "1.0"
        readme = "README.md"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        let path = Path::new("");
        let parse_error = TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    &Preview::default(),
                    Some(path),
                )
            })
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r###"
         × 'README.md' does not exist
          ╭─[pixi.toml:4:19]
        3 │         version = "1.0"
        4 │         readme = "README.md"
          ·                   ─────────
        5 │
          ╰────
        "###);
    }

    #[test]
    fn test_explicit_workspace_inheritance() {
        let input = r#"
        name = { workspace = true }
        version = { workspace = true }
        description = "Package description"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties {
            name: Some("workspace-name".to_string()),
            version: Some("1.0.0".parse().unwrap()),
            description: Some("Workspace description".to_string()),
            ..Default::default()
        };
        
        let manifest = package.into_manifest(workspace, &Preview::default(), None).unwrap();
        assert_eq!(manifest.value.package.name, "workspace-name");
        assert_eq!(manifest.value.package.version.to_string(), "1.0.0");
        assert_eq!(manifest.value.package.description, Some("Package description".to_string()));
    }

    #[test]
    fn test_invalid_workspace_false() {
        let input = r#"
        name = { workspace = false }
        version = "1.0.0"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        
        let parse_error = TomlPackage::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error));
    }

    #[test]
    fn test_missing_name_no_inheritance() {
        let input = r#"
        version = "1.0.0"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties::default();
        
        let parse_error = package.into_manifest(workspace, &Preview::default(), None).unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error));
    }

    #[test] 
    fn test_mixed_inheritance_and_direct_values() {
        let input = r#"
        name = { workspace = true }
        version = "2.0.0"
        description = { workspace = true }
        authors = ["Direct Author"]

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties {
            name: Some("workspace-name".to_string()),
            version: Some("1.0.0".parse().unwrap()),
            description: Some("Workspace description".to_string()),
            authors: Some(vec!["Workspace Author".to_string()]),
            ..Default::default()
        };
        
        let manifest = package.into_manifest(workspace, &Preview::default(), None).unwrap();
        assert_eq!(manifest.value.package.name, "workspace-name");
        assert_eq!(manifest.value.package.version.to_string(), "2.0.0");
        assert_eq!(manifest.value.package.description, Some("Workspace description".to_string()));
        assert_eq!(manifest.value.package.authors, Some(vec!["Direct Author".to_string()]));
    }

    #[test]
    fn test_workspace_inheritance_missing_workspace_value() {
        let input = r#"
        name = { workspace = true }
        version = "1.0.0"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties {
            // name is missing from workspace
            version: Some("1.0.0".parse().unwrap()),
            ..Default::default()
        };
        
        let parse_error = package.into_manifest(workspace, &Preview::default(), None).unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error));
    }

    #[test]
    fn test_workspace_inheritance_missing_version_workspace_value() {
        let input = r#"
        name = "package-name"
        version = { workspace = true }

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties {
            name: Some("workspace-name".to_string()),
            // version is missing from workspace
            ..Default::default()
        };
        
        let parse_error = package.into_manifest(workspace, &Preview::default(), None).unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error));
    }
}
