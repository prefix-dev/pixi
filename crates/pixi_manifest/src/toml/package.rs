use std::path::{Path, PathBuf};

use indexmap::IndexMap;
pub use pixi_toml::TomlFromStr;
use pixi_toml::{DeserializeAs, Same, TomlIndexMap, TomlWith};
use rattler_conda_types::Version;
use thiserror::Error;
use toml_span::{DeserError, Span, Spanned, Value, de_helpers::TableHelper};
use url::Url;

use crate::{
    PackageManifest, Preview, TargetSelector, Targets, TomlError, WithWarnings,
    error::GenericError,
    package::Package,
    toml::{
        TomlPackageBuild, manifest::ExternalWorkspaceProperties, package_target::TomlPackageTarget,
    },
    utils::{PixiSpanned, package_map::UniquePackageMap},
};

/// Represents a field that can either have a direct value or inherit from
/// workspace
#[derive(Debug, Clone)]
pub enum WorkspaceInheritableField<T> {
    /// Direct value specified in the package
    Value(T),
    /// Inherit the value from the workspace
    Workspace(Span),
    /// Do NOT inherit from workspace.
    /// This is an invalid case but to provide a nice error upstream we provide
    /// this here.
    NotWorkspace(Span),
}

impl<T> WorkspaceInheritableField<T> {
    /// Get the value if it's a direct value, otherwise return None
    pub fn value(self) -> Option<T> {
        match self {
            WorkspaceInheritableField::Value(v) => Some(v),
            WorkspaceInheritableField::Workspace(_) => None,
            WorkspaceInheritableField::NotWorkspace(_) => None,
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
            WorkspaceInheritableField::Workspace(span) => {
                WorkspaceInheritableField::Workspace(span)
            }
            WorkspaceInheritableField::NotWorkspace(span) => {
                WorkspaceInheritableField::NotWorkspace(span)
            }
        }
    }
}

impl<'de, T> toml_span::Deserialize<'de> for WorkspaceInheritableField<T>
where
    T: toml_span::Deserialize<'de>,
{
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        // First check if it's a table without consuming the value, then take it if it
        // is
        if value.as_table().is_some() {
            let mut th = TableHelper::new(value)?;
            let workspace = th.optional::<Spanned<bool>>("workspace");
            th.finalize(None)?;

            if let Some(Spanned { value: true, .. }) = workspace {
                return Ok(WorkspaceInheritableField::Workspace(value.span));
            } else if let Some(Spanned { value: false, span }) = workspace {
                return Ok(WorkspaceInheritableField::NotWorkspace(span));
            }
        }

        // If not a table or not { workspace = true }, try to deserialize as direct
        // value
        T::deserialize(value).map(WorkspaceInheritableField::Value)
    }
}

impl<'de, T, U> DeserializeAs<'de, WorkspaceInheritableField<T>> for WorkspaceInheritableField<U>
where
    U: DeserializeAs<'de, T>,
{
    fn deserialize_as(value: &mut Value<'de>) -> Result<WorkspaceInheritableField<T>, DeserError> {
        // First check if it's a table without consuming the value, then take it if it
        // is
        if value.as_table().is_some() {
            let mut th = TableHelper::new(value)?;
            let workspace = th.optional::<Spanned<bool>>("workspace");
            th.finalize(None)?;

            if let Some(Spanned { value: true, .. }) = workspace {
                return Ok(WorkspaceInheritableField::Workspace(value.span));
            } else if let Some(Spanned { value: false, span }) = workspace {
                return Ok(WorkspaceInheritableField::NotWorkspace(span));
            }
        }

        // If not a table or not { workspace = true }, try to deserialize as direct
        // value
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

/// Defines package defaults that can be used as fallback values.
///
/// This contains the package-level defaults (e.g., from `[project]` section in
/// pyproject.toml).
#[derive(Debug, Clone, Default)]
pub struct PackageDefaults {
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
    /// Helper function to resolve an optional field with 3-tier hierarchy:
    /// 1. Direct value (from package)
    /// 2. Workspace inheritance (from workspace) - ERROR if explicitly
    ///    requested but missing
    /// 3. Package defaults (from [project] section if the manifest is a
    ///    `pyproject.toml`)
    /// 4. None if missing at all levels
    fn resolve_optional_field_with_defaults<T>(
        field: Option<WorkspaceInheritableField<T>>,
        workspace_value: Option<T>,
        default_value: Option<T>,
        field_name: &'static str,
    ) -> Result<Option<T>, TomlError> {
        match field {
            Some(WorkspaceInheritableField::Value(v)) => Ok(Some(v)),
            Some(WorkspaceInheritableField::Workspace(span)) => {
                // If workspace inheritance is explicitly requested, the workspace must provide
                // the value
                match workspace_value {
                    Some(value) => Ok(Some(value)),
                    None => Err(GenericError::new(format!(
                        "the workspace does not define a '{}'",
                        field_name
                    ))
                    .with_span(span.into())
                    .into()),
                }
            }
            Some(WorkspaceInheritableField::NotWorkspace(span)) => {
                Err(workspace_cannot_be_false().with_span(span.into()).into())
            }
            None => Ok(default_value),
        }
    }

    /// The `root_directory` is used to resolve relative paths, if it is `None`,
    /// paths are not checked.
    pub fn into_manifest(
        self,
        workspace: WorkspacePackageProperties,
        package_defaults: PackageDefaults,
        preview: &Preview,
        root_directory: Option<&Path>,
    ) -> Result<WithWarnings<PackageManifest>, TomlError> {
        let mut warnings = Vec::new();

        let build_result = self.build.into_build_system()?;
        warnings.extend(build_result.warnings);

        // Resolve fields with 3-tier hierarchy: direct → workspace → package defaults →
        // error
        let name = Self::resolve_optional_field_with_defaults(
            self.name,
            workspace.name,
            package_defaults.name,
            "name",
        )?;
        let version = Self::resolve_optional_field_with_defaults(
            self.version,
            workspace.version,
            package_defaults.version,
            "version",
        )?;

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

        // Check file existence for resolved paths with 3-tier hierarchy
        fn check_resolved_file(
            root_directory: Option<&Path>,
            field: Option<WorkspaceInheritableField<Spanned<PathBuf>>>,
            workspace_value: Option<PathBuf>,
            default_value: Option<PathBuf>,
        ) -> Result<Option<PathBuf>, TomlError> {
            let Some(root_directory) = root_directory else {
                return Ok(None);
            };
            match field {
                None => {
                    // Fall back to package defaults
                    Ok(default_value.and_then(|value| pathdiff::diff_paths(value, root_directory)))
                }
                Some(WorkspaceInheritableField::Workspace(_)) => {
                    Ok(workspace_value
                        .and_then(|value| pathdiff::diff_paths(value, root_directory)))
                }
                Some(WorkspaceInheritableField::NotWorkspace(span)) => {
                    Err(workspace_cannot_be_false().with_span(span.into()).into())
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

        let license_file = check_resolved_file(
            root_directory,
            self.license_file,
            workspace.license_file,
            package_defaults.license_file,
        )?;
        let readme = check_resolved_file(
            root_directory,
            self.readme,
            workspace.readme,
            package_defaults.readme,
        )?;

        Ok(WithWarnings::from(PackageManifest {
            package: Package {
                name,
                version,
                description: Self::resolve_optional_field_with_defaults(
                    self.description,
                    workspace.description,
                    package_defaults.description,
                    "description",
                )?,
                authors: Self::resolve_optional_field_with_defaults(
                    self.authors,
                    workspace.authors,
                    package_defaults.authors,
                    "authors",
                )?,
                license: Self::resolve_optional_field_with_defaults(
                    self.license.map(|field| field.map(Spanned::take)),
                    workspace.license,
                    package_defaults.license,
                    "license",
                )?,
                license_file,
                readme,
                homepage: Self::resolve_optional_field_with_defaults(
                    self.homepage,
                    workspace.homepage,
                    package_defaults.homepage,
                    "homepage",
                )?,
                repository: Self::resolve_optional_field_with_defaults(
                    self.repository,
                    workspace.repository,
                    package_defaults.repository,
                    "repository",
                )?,
                documentation: Self::resolve_optional_field_with_defaults(
                    self.documentation,
                    workspace.documentation,
                    package_defaults.documentation,
                    "documentation",
                )?,
            },
            build: build_result.value,
            targets: Targets::from_default_and_user_defined(default_package_target, targets),
        })
        .with_warnings(warnings))
    }
}

fn workspace_cannot_be_false() -> GenericError {
    GenericError::new("`workspace` cannot be false")
        .with_help("By default no fields are inherited from the workspace")
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;

    use super::*;
    use crate::toml::FromTomlStr;

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
                    PackageDefaults::default(),
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
                    PackageDefaults::default(),
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

        let manifest = package
            .into_manifest(
                workspace,
                PackageDefaults::default(),
                &Preview::default(),
                None,
            )
            .unwrap();
        assert_eq!(manifest.value.package.name.unwrap(), "workspace-name");
        assert_eq!(manifest.value.package.version.unwrap().to_string(), "1.0.0");
        assert_eq!(
            manifest.value.package.description,
            Some("Package description".to_string())
        );
    }

    #[test]
    fn test_invalid_workspace_false() {
        let input = r#"
        name = { workspace = false }
        version = "1.0.0"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let package = TomlPackage::from_toml_str(input).unwrap();
        assert_matches!(
            package.name,
            Some(WorkspaceInheritableField::NotWorkspace(_))
        );
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

        let manifest = package
            .into_manifest(
                workspace,
                PackageDefaults::default(),
                &Preview::default(),
                None,
            )
            .unwrap();
        assert_eq!(manifest.value.package.name.unwrap(), "workspace-name");
        assert_eq!(manifest.value.package.version.unwrap().to_string(), "2.0.0");
        assert_eq!(
            manifest.value.package.description,
            Some("Workspace description".to_string())
        );
        assert_eq!(
            manifest.value.package.authors,
            Some(vec!["Direct Author".to_string()])
        );
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

        let parse_error = package
            .into_manifest(
                workspace,
                PackageDefaults::default(),
                &Preview::default(),
                None,
            )
            .unwrap_err();
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

        let parse_error = package
            .into_manifest(
                workspace,
                PackageDefaults::default(),
                &Preview::default(),
                None,
            )
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error));
    }

    #[test]
    fn test_package_defaults_3tier_hierarchy() {
        let input = r#"
        description = "Package description"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties::default(); // Empty workspace
        let package_defaults = PackageDefaults {
            name: Some("default-name".to_string()),
            version: Some("2.0.0".parse().unwrap()),
            description: Some("Default description".to_string()),
            authors: Some(vec!["Default Author".to_string()]),
            ..Default::default()
        };

        let manifest = package
            .into_manifest(workspace, package_defaults, &Preview::default(), None)
            .unwrap();
        // Should use package defaults for name and version
        assert_eq!(manifest.value.package.name.unwrap(), "default-name");
        assert_eq!(manifest.value.package.version.unwrap().to_string(), "2.0.0");
        // Should use direct value for description
        assert_eq!(
            manifest.value.package.description,
            Some("Package description".to_string())
        );
        // Should use package defaults for authors
        assert_eq!(
            manifest.value.package.authors,
            Some(vec!["Default Author".to_string()])
        );
    }

    #[test]
    fn test_workspace_inheritance_overrides_package_defaults() {
        let input = r#"
        name = { workspace = true }
        version = { workspace = true }

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties {
            name: Some("workspace-name".to_string()),
            version: Some("3.0.0".parse().unwrap()),
            ..Default::default()
        };
        let package_defaults = PackageDefaults {
            name: Some("default-name".to_string()),
            version: Some("2.0.0".parse().unwrap()),
            description: Some("Default description".to_string()),
            ..Default::default()
        };

        let manifest = package
            .into_manifest(workspace, package_defaults, &Preview::default(), None)
            .unwrap();
        // Should use workspace values for name and version (overrides defaults)
        assert_eq!(manifest.value.package.name.unwrap(), "workspace-name");
        assert_eq!(manifest.value.package.version.unwrap().to_string(), "3.0.0");
        // Should use package defaults for description (not specified anywhere else)
        assert_eq!(
            manifest.value.package.description,
            Some("Default description".to_string())
        );
    }

    #[test]
    fn test_optional_workspace_inheritance_missing_workspace_value() {
        let input = r#"
        name = "package-name"
        version = "1.0.0"
        description = { workspace = true }

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties {
            name: Some("workspace-name".to_string()),
            version: Some("1.0.0".parse().unwrap()),
            // description is missing from workspace
            ..Default::default()
        };
        let package_defaults = PackageDefaults {
            description: Some("Default description".to_string()),
            ..Default::default()
        };

        let parse_error = package
            .into_manifest(workspace, package_defaults, &Preview::default(), None)
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error));
    }

    #[test]
    fn test_target_specific_build_config() {
        let input = r#"
        name = "package-name"
        version = "1.0.0"
        
        [build.config]
        test = "test_normal"
        
        [build.target.unix.config]
        test = "test_unix"
        
        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties::default();

        let parsed = package
            .into_manifest(
                workspace,
                PackageDefaults::default(),
                &Preview::default(),
                None,
            )
            .unwrap();

        // Now check if we can also parse the deprecated `configuration` key
        let input = r#"
        name = "package-name"
        version = "1.0.0"
        
        [build.configuration]
        test = "test_normal"
        
        [build.target.unix.configuration]
        test = "test_unix"
        
        [build]
        backend = { name = "bla", version = "1.0" }
        "#;
        let package = TomlPackage::from_toml_str(input).unwrap();
        let workspace = WorkspacePackageProperties::default();

        let parsed_deprecated = package
            .into_manifest(
                workspace,
                PackageDefaults::default(),
                &Preview::default(),
                None,
            )
            .unwrap();

        assert!(!parsed_deprecated.warnings.is_empty());
        assert_eq!(parsed.value.build, parsed_deprecated.value.build);
    }
}
