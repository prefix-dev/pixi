use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use pixi_build_types::ConditionalExpression;
use pixi_spec::TomlSpec;
pub use pixi_toml::TomlFromStr;
use pixi_toml::{DeserializeAs, Same, TomlIndexMap, TomlWith};
use rattler_conda_types::{PackageName, Version};
use thiserror::Error;
use toml_span::{DeserError, Span, Spanned, Value, de_helpers::TableHelper};
use url::Url;

use crate::{
    PackageManifest, Preview, TargetSelector, TomlError, WithWarnings,
    error::GenericError,
    package::Package,
    target::PackageTarget,
    toml::{
        TomlPackageBuild, manifest::ExternalWorkspaceProperties, package_target::TomlPackageTarget,
    },
    utils::{
        PixiSpanned,
        inheritable_package_map::{
            ConditionalInheritablePackageMap, ConditionalSpecs, InheritablePackageMap,
        },
    },
    warning::Deprecation,
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
    pub host_dependencies: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub build_dependencies: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub run_dependencies: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
    pub extra_dependencies:
        IndexMap<PixiSpanned<String>, PixiSpanned<ConditionalInheritablePackageMap>>,
    pub run_constraints: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
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
        let extra_dependencies = th
            .optional::<TomlWith<_, TomlIndexMap<_, Same>>>("extra-dependencies")
            .map(TomlWith::into_inner)
            .unwrap_or_default();
        let run_constraints = th.optional("run-constraints");
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
            extra_dependencies,
            run_constraints,
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
    /// `[workspace.dependencies]` pool; paths are relative to `workspace_root`.
    pub dependencies: IndexMap<PackageName, TomlSpec>,

    /// Absolute directory of the workspace manifest. Used to re-base
    /// `dependencies` path specs against the member's directory.
    pub workspace_root: Option<PathBuf>,
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
            dependencies: IndexMap::new(),
            workspace_root: None,
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
    /// 3. Package defaults (from `[project]` section if the manifest is a
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
                        "the workspace does not define a '{field_name}'"
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
        root_directory: &Path,
    ) -> Result<WithWarnings<PackageManifest>, TomlError> {
        let mut warnings = Vec::new();

        // Re-base workspace dependency path specs against this member's
        // directory. The pool itself stores them relative to the workspace root.
        let workspace_dependencies = rebase_workspace_path_specs(
            &workspace.dependencies,
            workspace.workspace_root.as_deref(),
            root_directory,
        );

        let build_result = self.build.into_build_system(&workspace_dependencies)?;
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

        // Split each package-level dependency table into its unconditional
        // entries and any `if(<expression>)` sub-tables.
        let (run_unconditional, run_conditional) = split_section(self.run_dependencies);
        let (constraints_unconditional, constraints_conditional) =
            split_section(self.run_constraints);
        let (host_unconditional, host_conditional) = split_section(self.host_dependencies);
        let (build_unconditional, build_conditional) = split_section(self.build_dependencies);

        let mut extra_unconditional: IndexMap<
            PixiSpanned<String>,
            PixiSpanned<InheritablePackageMap>,
        > = IndexMap::new();
        let mut extra_conditional: Vec<(PixiSpanned<String>, ConditionalSpecs)> = Vec::new();
        for (group, PixiSpanned { value, span }) in self.extra_dependencies {
            let (unconditional, conditional) = value.into_parts();
            if !unconditional.is_empty() {
                extra_unconditional.insert(
                    group.clone(),
                    PixiSpanned {
                        value: unconditional,
                        span,
                    },
                );
            }
            for spec in conditional {
                extra_conditional.push((group.clone(), spec));
            }
        }

        // Unconditional entries form the default target.
        let default_package_target = TomlPackageTarget {
            run_dependencies: run_unconditional,
            run_constraints: constraints_unconditional,
            host_dependencies: host_unconditional,
            build_dependencies: build_unconditional,
            extra_dependencies: extra_unconditional,
        }
        .into_package_target(preview, &workspace_dependencies)?;

        // Fold the conditional sub-tables into one `TomlPackageTarget` per
        // distinct expression, merging across the dependency sections.
        type SectionField =
            fn(&mut TomlPackageTarget) -> &mut Option<PixiSpanned<InheritablePackageMap>>;
        let sections: [(Vec<ConditionalSpecs>, SectionField); 4] = [
            (run_conditional, |target| &mut target.run_dependencies),
            (constraints_conditional, |target| {
                &mut target.run_constraints
            }),
            (host_conditional, |target| &mut target.host_dependencies),
            (build_conditional, |target| &mut target.build_dependencies),
        ];
        let mut conditional_targets: IndexMap<ConditionalExpression, TomlPackageTarget> =
            IndexMap::new();
        for (specs, field) in sections {
            for spec in specs {
                *field(conditional_targets.entry(spec.expression).or_default()) =
                    Some(PixiSpanned {
                        value: spec.specs,
                        span: Some(spec.value_span),
                    });
            }
        }
        for (group, spec) in extra_conditional {
            conditional_targets
                .entry(spec.expression)
                .or_default()
                .extra_dependencies
                .insert(
                    group,
                    PixiSpanned {
                        value: spec.specs,
                        span: Some(spec.value_span),
                    },
                );
        }

        // The legacy `[package.target.PLATFORM]` syntax is deprecated but still
        // supported: each table lowers to the conditional dependency tables for
        // the equivalent expression. Emit a deprecation warning that spells out
        // that replacement.
        for (selector, toml_target) in self.target {
            let expression = target_selector_expression(&selector.value);
            warnings.push(
                Deprecation::package_target(
                    package_target_replacement_help(expression.as_str(), &toml_target),
                    selector.span.clone(),
                )
                .into(),
            );
            if conditional_targets.contains_key(&expression) {
                return Err(GenericError::new(format!(
                    "duplicate condition: `[package.target.{}]` is equivalent to `\"if({expression})\"`",
                    selector.value
                ))
                .with_opt_span(selector.span)
                .with_span_label("this target table lowers to the same condition")
                .with_help(format!(
                    "Move the dependencies into the `\"if({expression})\"` tables and remove the `[package.target.{}]` table",
                    selector.value
                ))
                .into());
            }
            conditional_targets.insert(expression, toml_target);
        }

        // `if(...)` conditionals are not platform selectors; they are kept
        // separate and passed through to rattler-build, which evaluates the
        // expression.
        let mut conditional_dependencies: IndexMap<ConditionalExpression, PackageTarget> =
            IndexMap::new();
        for (expression, toml_target) in conditional_targets {
            let target = toml_target.into_package_target(preview, &workspace_dependencies)?;
            conditional_dependencies.insert(expression, target);
        }

        if let Some(WorkspaceInheritableField::Value(Spanned {
            value: license,
            span,
        })) = &self.license
            && let Err(e) = spdx::Expression::parse(license)
        {
            return Err(
                GenericError::new("'license' is not a valid SPDX expression")
                    .with_span((*span).into())
                    .with_span_label(e.to_string())
                    .into(),
            );
        }

        // Check file existence for resolved paths with 3-tier hierarchy.
        // If root_directory is None, validation is skipped.
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

        // Determine the directory to use for file validation based on build.source:
        // - If build.source is a git or URL source, pass None to skip validation (files are remote)
        // - If build.source is a path source, resolve the path and validate against that directory
        // - If build.source is not set, validate against the manifest directory
        let file_validation_dir: Option<PathBuf> =
            match (&build_result.value.source, root_directory) {
                // Git or URL source: skip validation (files are in remote location)
                (Some(pixi_spec::SourceLocationSpec::Git(_)), _)
                | (Some(pixi_spec::SourceLocationSpec::Url(_)), _) => None,
                // Path source: resolve the path and use that directory for validation
                (Some(pixi_spec::SourceLocationSpec::Path(path_spec)), root_dir) => {
                    path_spec.resolve(root_dir).ok()
                }
                // No source: use the manifest directory
                (None, root_dir) => Some(root_dir.to_path_buf()),
                // No root directory provided: skip validation
            };

        let license_file = check_resolved_file(
            file_validation_dir.as_deref(),
            self.license_file,
            workspace.license_file,
            package_defaults.license_file,
        )?;
        let readme = check_resolved_file(
            file_validation_dir.as_deref(),
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
            dependencies: default_package_target,
            conditional_dependencies,
        })
        .with_warnings(warnings))
    }
}

/// The conditional expression a deprecated `[package.target.SELECTOR]` table
/// lowers to.
///
/// Platform selectors map to `host_platform == '<platform>'` (the behavior the
/// legacy syntax already had); family selectors (`unix`/`linux`/`win`/`osx`)
/// map to the bare rattler-build boolean of the same name. Note that the
/// `macos` alias maps to `osx`, the only spelling defined in rattler-build's
/// jinja context.
fn target_selector_expression(selector: &TargetSelector) -> ConditionalExpression {
    match selector {
        TargetSelector::Platform(_) | TargetSelector::Subdir(_) => {
            ConditionalExpression::new(format!("host_platform == '{selector}'"))
        }
        other => ConditionalExpression::new(other.to_string()),
    }
}

/// Build the tailored `help` text suggesting the conditional dependency tables
/// that replace a deprecated `[package.target.SELECTOR]` entry.
fn package_target_replacement_help(expression: &str, toml_target: &TomlPackageTarget) -> String {
    let mut lines = Vec::new();
    let mut push_line = |section: &str| {
        lines.push(format!("  [package.{section}.\"if({expression})\"]"));
    };
    if toml_target.build_dependencies.is_some() {
        push_line("build-dependencies");
    }
    if toml_target.host_dependencies.is_some() {
        push_line("host-dependencies");
    }
    if toml_target.run_dependencies.is_some() {
        push_line("run-dependencies");
    }
    if toml_target.run_constraints.is_some() {
        push_line("run-constraints");
    }
    if !toml_target.extra_dependencies.is_empty() {
        lines.push(format!(
            "  [package.extra-dependencies.<group>.\"if({expression})\"]"
        ));
    }

    format!(
        "Move the dependencies under a conditional dependency table instead:\n{}",
        lines.join("\n")
    )
}

/// Split a package-level dependency table into its unconditional entries (which
/// belong to the default target) and the list of `if(<expression>)` sub-tables.
/// Returns `None` for the unconditional part when it is empty.
fn split_section(
    field: Option<PixiSpanned<ConditionalInheritablePackageMap>>,
) -> (
    Option<PixiSpanned<InheritablePackageMap>>,
    Vec<ConditionalSpecs>,
) {
    match field {
        None => (None, Vec::new()),
        Some(PixiSpanned { value, span }) => {
            let (unconditional, conditional) = value.into_parts();
            let unconditional = (!unconditional.is_empty()).then_some(PixiSpanned {
                value: unconditional,
                span,
            });
            (unconditional, conditional)
        }
    }
}

fn workspace_cannot_be_false() -> GenericError {
    GenericError::new("`workspace` cannot be false")
        .with_help("By default no fields are inherited from the workspace")
}

/// Re-anchor `path` entries in workspace `TomlSpec`s from `workspace_root` to
/// `member_root`. Absolute and `~/` paths are returned unchanged. Returns the
/// input map verbatim when either root is unknown or the roots are equal.
fn rebase_workspace_path_specs(
    specs: &IndexMap<PackageName, TomlSpec>,
    workspace_root: Option<&Path>,
    member_root: &Path,
) -> IndexMap<PackageName, TomlSpec> {
    let Some(workspace_root) = workspace_root else {
        return specs.clone();
    };
    if workspace_root == member_root
        || workspace_root.as_os_str().is_empty()
        || member_root.as_os_str().is_empty()
    {
        return specs.clone();
    }
    specs
        .iter()
        .map(|(name, spec)| {
            let mut rebased = spec.clone();
            rebased.rebase_path(workspace_root, member_root);
            (name.clone(), rebased)
        })
        .collect()
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use assert_matches::assert_matches;
    use fs_err as fs;
    use insta::assert_snapshot;
    use pixi_spec::PixiSpec;
    use pixi_test_utils::format_parse_error;
    use rattler_conda_types::PackageName;
    use tempfile::TempDir;

    use super::*;
    use crate::{KnownPreviewFeature, SpecType, toml::FromTomlStr};
    use pixi_build_types::ConditionalExpression;

    /// Parses a manifest using only `Preview::default()` and asserts it succeeds.
    fn parse_package(input: &str) -> PackageManifest {
        TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    PackageDefaults::default(),
                    &Preview::default(),
                    Path::new(""),
                )
            })
            .expect("expected manifest to parse")
            .value
    }

    /// Asserts that the dependency map for `spec_type` contains exactly one
    /// entry for `name` whose version spec stringifies to `expected`.
    #[track_caller]
    fn assert_single_version(
        deps: &std::collections::HashMap<
            SpecType,
            pixi_spec_containers::DependencyMap<PackageName, PixiSpec>,
        >,
        spec_type: SpecType,
        name: &str,
        expected: &str,
    ) {
        let entry = deps
            .get(&spec_type)
            .unwrap_or_else(|| panic!("missing {spec_type:?} bucket"));
        let specs = entry
            .get(&PackageName::from_str(name).unwrap())
            .unwrap_or_else(|| panic!("missing {name} in {spec_type:?}"));
        assert_eq!(specs.len(), 1, "expected exactly one spec for {name}");
        assert_eq!(
            specs
                .iter()
                .next()
                .unwrap()
                .as_version_spec()
                .unwrap()
                .to_string(),
            expected,
        );
    }

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
                    path,
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
                    path,
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
    fn test_package_extras_dependencies() {
        let input = r#"
        name = "bla"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [extra-dependencies.test]
        gtest = "*"
        pytest = ">=8"
        "#;

        let package = TomlPackage::from_toml_str(input).unwrap();
        let manifest = package
            .into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                Path::new(""),
            )
            .unwrap()
            .value;

        let test_extra = manifest
            .dependencies
            .extra_dependencies
            .get("test")
            .expect("test extra exists");
        let names = test_extra
            .names()
            .map(|name| name.as_normalized())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["gtest", "pytest"]);
    }

    #[test]
    fn test_package_target_extras_dependencies() {
        // Per-target extras should land on the matching package target rather
        // than on the default target.
        let input = r#"
        name = "bla"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [target.win.extra-dependencies.test]
        gtest = "*"

        [target.win.extra-dependencies.bench]
        criterion = "*"
        "#;

        let package = TomlPackage::from_toml_str(input).unwrap();
        let manifest = package
            .into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                Path::new(""),
            )
            .unwrap()
            .value;

        let win_target = manifest
            .conditional_dependencies
            .get(&ConditionalExpression::new("win"))
            .expect("win target exists");
        assert!(win_target.extra_dependencies.contains_key("test"));
        assert!(win_target.extra_dependencies.contains_key("bench"));
        // Default target should NOT have the per-target extras.
        assert!(manifest.dependencies.extra_dependencies.is_empty());
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
                Path::new(""),
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
                Path::new(""),
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
                Path::new(""),
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
                Path::new(""),
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
            .into_manifest(
                workspace,
                package_defaults,
                &Preview::default(),
                Path::new(""),
            )
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
            .into_manifest(
                workspace,
                package_defaults,
                &Preview::default(),
                Path::new(""),
            )
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
            .into_manifest(
                workspace,
                package_defaults,
                &Preview::default(),
                Path::new(""),
            )
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
                Path::new(""),
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
                Path::new(""),
            )
            .unwrap();

        assert!(!parsed_deprecated.warnings.is_empty());
        assert_eq!(parsed.value.build, parsed_deprecated.value.build);
    }

    #[test]
    fn test_license_file_validation_skipped_for_git_source() {
        // When build.source is a git source, license-file validation should be skipped
        // because the file will be in the checked-out source directory, not the manifest directory.
        let input = r#"
        name = "bla"
        version = "1.0"
        license-file = "LICENSE.txt"

        [build]
        backend = { name = "bla", version = "1.0" }
        source = { git = "https://github.com/example/repo", rev = "abc123" }
        "#;
        let path = Path::new("");
        // This should NOT fail even though LICENSE.txt doesn't exist,
        // because the source is a git repository.
        let result = TomlPackage::from_toml_str(input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                path,
            )
        });
        assert!(result.is_ok(), "Expected success but got: {result:?}");
    }

    #[test]
    fn test_license_file_validation_skipped_for_url_source() {
        // When build.source is a URL source, license-file validation should be skipped
        // because the file will be in the downloaded/extracted source directory.
        let input = r#"
        name = "bla"
        version = "1.0"
        license-file = "LICENSE.txt"

        [build]
        backend = { name = "bla", version = "1.0" }
        source = { url = "https://example.com/archive.tar.gz" }
        "#;
        let path = Path::new("");
        // This should NOT fail even though LICENSE.txt doesn't exist,
        // because the source is a URL.
        let result = TomlPackage::from_toml_str(input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                path,
            )
        });
        assert!(result.is_ok(), "Expected success but got: {result:?}");
    }

    #[test]
    fn test_readme_validation_skipped_for_git_source() {
        // When build.source is a git source, readme validation should be skipped
        let input = r#"
        name = "bla"
        version = "1.0"
        readme = "README.md"

        [build]
        backend = { name = "bla", version = "1.0" }
        source = { git = "https://github.com/example/repo", branch = "main" }
        "#;
        let path = Path::new("");
        // This should NOT fail even though README.md doesn't exist
        let result = TomlPackage::from_toml_str(input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                path,
            )
        });
        assert!(result.is_ok(), "Expected success but got: {result:?}");
    }

    #[test]
    fn test_license_file_validation_fails_for_path_source_missing_file() {
        // When build.source is a path source, license-file validation should still run
        // and fail if the file doesn't exist in the source directory
        let input = r#"
        name = "bla"
        version = "1.0"
        license-file = "LICENSE.txt"

        [build]
        backend = { name = "bla", version = "1.0" }
        source = { path = "../some/path" }
        "#;
        let path = Path::new("");
        // This should fail because LICENSE.txt doesn't exist and source is a path
        let result = TomlPackage::from_toml_str(input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                path,
            )
        });
        assert!(result.is_err(), "Expected failure for path source");
    }

    #[test]
    fn test_license_file_validation_succeeds_without_build_source() {
        // When no build.source is specified, license-file should be validated
        // against the manifest directory
        let temp_dir = TempDir::new().unwrap();
        let license_path = temp_dir.path().join("LICENSE.txt");
        fs::write(&license_path, "MIT License").unwrap();

        let input = r#"
        name = "bla"
        version = "1.0"
        license-file = "LICENSE.txt"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let result = TomlPackage::from_toml_str(input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                temp_dir.path(),
            )
        });
        assert!(result.is_ok(), "Expected success but got: {result:?}");

        // Verify the license_file path is set correctly
        let manifest = result.unwrap().value;
        assert!(manifest.package.license_file.is_some());
    }

    #[test]
    fn test_license_file_validation_succeeds_with_path_source() {
        // When build.source is a path, license-file should be validated
        // against the resolved path source directory
        // Create manifest directory
        let manifest_dir = TempDir::new().unwrap();

        // Create source directory with license file
        let source_dir = TempDir::new().unwrap();
        let license_path = source_dir.path().join("LICENSE.txt");
        fs::write(&license_path, "MIT License").unwrap();

        // Use the source directory path in the manifest.
        // Replace backslashes with forward slashes for Windows compatibility in TOML strings.
        let source_path = source_dir.path().to_string_lossy().replace('\\', "/");
        let input = format!(
            r#"
        name = "bla"
        version = "1.0"
        license-file = "LICENSE.txt"

        [build]
        backend = {{ name = "bla", version = "1.0" }}
        source = {{ path = "{source_path}" }}
        "#
        );

        let result = TomlPackage::from_toml_str(&input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                manifest_dir.path(),
            )
        });
        assert!(result.is_ok(), "Expected success but got: {result:?}");

        // Verify the license_file path is set correctly
        let manifest = result.unwrap().value;
        assert!(manifest.package.license_file.is_some());
    }

    #[test]
    fn test_package_dependencies_all_types() {
        // Each of run-, host-, build-dependencies and run-constraints at the
        // package level must land in its own SpecType bucket of the default target.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [run-dependencies]
        run-dep = "==1.0"

        [host-dependencies]
        host-dep = "==2.0"

        [build-dependencies]
        build-dep = "==3.0"

        [run-constraints]
        constrained = ">=4.0"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let manifest = parse_package(input);
        let deps = &manifest.dependencies.dependencies;

        assert_single_version(deps, SpecType::Run, "run-dep", "==1.0");
        assert_single_version(deps, SpecType::Host, "host-dep", "==2.0");
        assert_single_version(deps, SpecType::Build, "build-dep", "==3.0");
        assert_single_version(deps, SpecType::RunConstraints, "constrained", ">=4.0");
    }

    #[test]
    fn test_package_target_specific_dependencies() {
        // Target-specific package dependencies (including run-constraints) must
        // land in the per-target bucket, not the default target.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [run-dependencies]
        shared = "==1.0"

        [target.linux-64.run-dependencies]
        only-linux = "==2.0"

        [target.linux-64.run-constraints]
        only-linux-constrained = ">=3.0"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let manifest = parse_package(input);

        // Default target only has the shared run dep.
        let default_deps = &manifest.dependencies.dependencies;
        assert_single_version(default_deps, SpecType::Run, "shared", "==1.0");
        assert!(
            !default_deps.contains_key(&SpecType::RunConstraints),
            "run-constraints should not leak into default target",
        );

        // The linux-64 target lowers to the equivalent conditional expression.
        let linux = manifest
            .conditional_dependencies
            .get(&ConditionalExpression::new("host_platform == 'linux-64'"))
            .expect("linux-64 target should exist");
        assert_single_version(&linux.dependencies, SpecType::Run, "only-linux", "==2.0");
        assert_single_version(
            &linux.dependencies,
            SpecType::RunConstraints,
            "only-linux-constrained",
            ">=3.0",
        );
    }

    #[test]
    fn test_package_conditional_dependencies() {
        // `if(<expression>)` keys inside the package dependency tables become
        // `Expression` targets; plain entries stay on the default target.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [run-dependencies]
        shared = "==1.0"

        [build-dependencies."if(host_platform != build_platform)"]
        cross-tool = "==2.0"

        [host-dependencies."if(host_platform == 'linux-64')"]
        libgl = "==3.0"
        "#;

        let manifest = parse_package(input);

        // Plain entry stays on the default target.
        assert_single_version(
            &manifest.dependencies.dependencies,
            SpecType::Run,
            "shared",
            "==1.0",
        );

        // Each `if(...)` block produces a conditional target with only the
        // matching dependency bucket populated.
        let cross = manifest
            .conditional_dependencies
            .get(&ConditionalExpression::new(
                "host_platform != build_platform",
            ))
            .expect("conditional target should exist");
        assert_single_version(&cross.dependencies, SpecType::Build, "cross-tool", "==2.0");

        let linux = manifest
            .conditional_dependencies
            .get(&ConditionalExpression::new("host_platform == 'linux-64'"))
            .expect("conditional target should exist");
        assert_single_version(&linux.dependencies, SpecType::Host, "libgl", "==3.0");
    }

    #[test]
    fn test_package_conditional_merges_same_expression() {
        // The same expression used across sections folds into a single target.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [build-dependencies."if(host_platform == 'linux-64')"]
        build-only = "==1.0"

        [run-dependencies."if(host_platform == 'linux-64')"]
        run-only = "==2.0"
        "#;

        let manifest = parse_package(input);
        assert_eq!(
            manifest.conditional_dependencies.len(),
            1,
            "the two sections must merge into one target"
        );

        let target = manifest
            .conditional_dependencies
            .get(&ConditionalExpression::new("host_platform == 'linux-64'"))
            .expect("conditional target should exist");
        assert_single_version(&target.dependencies, SpecType::Build, "build-only", "==1.0");
        assert_single_version(&target.dependencies, SpecType::Run, "run-only", "==2.0");
    }

    #[test]
    fn test_package_conditional_malformed_expression() {
        // A key containing `(` that is not a well-formed `if(...)` is rejected.
        assert_snapshot!(expect_parse_failure(
            r#"
        name = "pkg"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [build-dependencies."matches(python, '>=3.10')"]
        foo = "*"
        "#,
        ), @r###"
          × `matches(python, '>=3.10')` is not a valid selector. Wrap the expression in `if(...)`, e.g. `if(host_platform == 'linux-64')`
           ╭─[pixi.toml:8:30]
         7 │
         8 │         [build-dependencies."matches(python, '>=3.10')"]
           ·                              ─────────────────────────
         9 │         foo = "*"
           ╰────
        "###);
    }

    #[test]
    fn test_package_target_emits_deprecation_warning() {
        // The legacy `[package.target.*]` syntax still parses, but produces a
        // deprecation warning suggesting the conditional form.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [target.linux-64.build-dependencies]
        foo = "==1.0"
        "#;

        let mut parsed = TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    PackageDefaults::default(),
                    &Preview::default(),
                    Path::new(""),
                )
            })
            .expect("legacy target syntax must still parse");

        assert!(
            !parsed.warnings.is_empty(),
            "legacy target syntax must emit a deprecation warning"
        );
        // The rendered warning names the deprecated table and spells out the
        // exact conditional syntax to use instead.
        assert_snapshot!(
            format_parse_error(input, parsed.warnings.remove(0)),
            @r#"
         ⚠ the `[package.target]` tables are deprecated in favor of conditional dependencies
          ╭─[pixi.toml:8:17]
        7 │
        8 │         [target.linux-64.build-dependencies]
          ·                 ────┬───
          ·                     ╰── deprecated target selector
        9 │         foo = "==1.0"
          ╰────
         help: Move the dependencies under a conditional dependency table instead:
                 [package.build-dependencies."if(host_platform == 'linux-64')"]
        "#
        );

        // The legacy target still works: it lowers to the equivalent
        // conditional dependency entry.
        let linux = parsed
            .value
            .conditional_dependencies
            .get(&ConditionalExpression::new("host_platform == 'linux-64'"))
            .expect("linux-64 target should lower to a conditional entry");
        assert_single_version(&linux.dependencies, SpecType::Build, "foo", "==1.0");
    }

    #[test]
    fn test_package_target_collides_with_equivalent_conditional() {
        // A deprecated target table and an explicit `if(...)` table that lower
        // to the same expression are rejected; silently merging them would hide
        // a half-finished migration.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [run-dependencies."if(host_platform == 'linux-64')"]
        foo = "==1.0"

        [target.linux-64.build-dependencies]
        bar = "==2.0"
        "#;

        let parse_error = TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    PackageDefaults::default(),
                    &Preview::default(),
                    Path::new(""),
                )
            })
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r#"
         × duplicate condition: `[package.target.linux-64]` is equivalent to `"if(host_platform == 'linux-64')"`
           ╭─[pixi.toml:11:17]
        10 │
        11 │         [target.linux-64.build-dependencies]
           ·                 ────┬───
           ·                     ╰── this target table lowers to the same condition
        12 │         bar = "==2.0"
           ╰────
         help: Move the dependencies into the `"if(host_platform == 'linux-64')"` tables and remove the `[package.target.linux-64]` table
        "#);
    }

    #[test]
    fn test_package_target_osx_lowers_to_osx_expression() {
        // `[package.target.osx]` must lower to the rattler-build family boolean
        // `osx`; `macos` is not defined in rattler-build's jinja context and
        // would silently evaluate to false.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [build]
        backend = { name = "bla", version = "1.0" }

        [target.osx.run-dependencies]
        foo = "==1.0"
        "#;

        let manifest = parse_package(input);
        let target = manifest
            .conditional_dependencies
            .get(&ConditionalExpression::new("osx"))
            .expect("the osx target must lower to the `osx` conditional expression");
        assert_single_version(&target.dependencies, SpecType::Run, "foo", "==1.0");
    }

    #[test]
    fn test_run_constraints_source_spec_requires_pixi_build() {
        // Source specs in [package.run-constraints] must be rejected unless the
        // pixi-build preview is enabled — same gate as the other dependency
        // tables.
        let input = r#"
        name = "pkg"
        version = "1.0"

        [run-constraints]
        local-pkg = { path = "./local" }

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let err = TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    PackageDefaults::default(),
                    &Preview::default(),
                    Path::new(""),
                )
            })
            .unwrap_err();
        let rendered = format_parse_error(input, err);
        assert!(
            rendered.contains("pixi-build"),
            "expected pixi-build gating error, got: {rendered}"
        );

        // With pixi-build enabled the same input parses.
        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        TomlPackage::from_toml_str(input)
            .and_then(|w| {
                w.into_manifest(
                    WorkspacePackageProperties::default(),
                    PackageDefaults::default(),
                    &preview,
                    Path::new(""),
                )
            })
            .expect("source specs in run-constraints must be allowed when pixi-build is enabled");
    }

    #[test]
    fn test_readme_validation_succeeds_without_build_source() {
        // When no build.source is specified, readme should be validated
        // against the manifest directory
        let temp_dir = TempDir::new().unwrap();
        let readme_path = temp_dir.path().join("README.md");
        fs::write(&readme_path, "# My Package").unwrap();

        let input = r#"
        name = "bla"
        version = "1.0"
        readme = "README.md"

        [build]
        backend = { name = "bla", version = "1.0" }
        "#;

        let result = TomlPackage::from_toml_str(input).and_then(|w| {
            w.into_manifest(
                WorkspacePackageProperties::default(),
                PackageDefaults::default(),
                &Preview::default(),
                temp_dir.path(),
            )
        });
        assert!(result.is_ok(), "Expected success but got: {result:?}");

        // Verify the readme path is set correctly
        let manifest = result.unwrap().value;
        assert!(manifest.package.readme.is_some());
    }

    #[test]
    fn test_rebase_workspace_path_specs_relativizes_to_member() {
        use indexmap::IndexMap;
        let mut pool: IndexMap<rattler_conda_types::PackageName, TomlSpec> = IndexMap::new();
        pool.insert("local".parse().unwrap(), path_spec("../shared"));

        let workspace_root = Path::new("/ws");
        let member_root = Path::new("/ws/members/foo");
        let rebased = super::rebase_workspace_path_specs(&pool, Some(workspace_root), member_root);
        let spec = rebased
            .get(&rattler_conda_types::PackageName::from_str("local").unwrap())
            .unwrap();
        assert_eq!(
            spec.location.as_ref().unwrap().path.as_deref(),
            Some("../../../shared")
        );
    }

    #[test]
    fn test_rebase_workspace_path_specs_passes_absolute_through() {
        use indexmap::IndexMap;
        let mut pool: IndexMap<rattler_conda_types::PackageName, TomlSpec> = IndexMap::new();
        pool.insert("abs".parse().unwrap(), path_spec("/abs/path"));

        let rebased =
            super::rebase_workspace_path_specs(&pool, Some(Path::new("/ws")), Path::new("/ws/m"));
        let spec = rebased
            .get(&rattler_conda_types::PackageName::from_str("abs").unwrap())
            .unwrap();
        assert_eq!(
            spec.location.as_ref().unwrap().path.as_deref(),
            Some("/abs/path")
        );
    }

    #[test]
    fn test_rebase_no_op_when_roots_match() {
        use indexmap::IndexMap;
        let mut pool: IndexMap<rattler_conda_types::PackageName, TomlSpec> = IndexMap::new();
        pool.insert("local".parse().unwrap(), path_spec("../shared"));

        let same = Path::new("/ws");
        let rebased = super::rebase_workspace_path_specs(&pool, Some(same), same);
        let spec = rebased
            .get(&rattler_conda_types::PackageName::from_str("local").unwrap())
            .unwrap();
        assert_eq!(
            spec.location.as_ref().unwrap().path.as_deref(),
            Some("../shared")
        );
    }

    /// Construct a [`TomlSpec`] carrying only a path location.
    fn path_spec(path: &str) -> TomlSpec {
        let mut spec = TomlSpec::empty();
        spec.location = Some(pixi_spec::TomlLocationSpec {
            url: None,
            git: None,
            path: Some(path.to_string()),
            branch: None,
            rev: None,
            tag: None,
            subdirectory: None,
            md5: None,
            sha256: None,
        });
        spec
    }
}
