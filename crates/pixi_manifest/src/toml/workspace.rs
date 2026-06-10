use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use indexmap::{IndexMap, IndexSet};
use pixi_spec::{ExcludeNewer, TomlSpec, TomlVersionSpecStr};
use pixi_toml::{TomlFromStr, TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::{PackageName, Version, VersionSpec};
use std::str::FromStr;
use toml_span::{DeserError, Span, Spanned, Value, de_helpers::TableHelper, value::ValueInner};
use url::Url;

use crate::{
    KnownPreviewFeature, PixiPlatform, PrioritizedChannel, S3Options, TargetSelector, Targets,
    TomlError, WithWarnings, Workspace,
    error::GenericError,
    pypi::pypi_options::PypiOptions,
    toml::{
        manifest::ExternalWorkspaceProperties, platform::TomlPixiPlatform, preview::TomlPreview,
    },
    utils::PixiSpanned,
    workspace::{BuildVariantSource, ChannelPriority, CondaPypiMap, SolveStrategy},
};

/// Parses `[workspace.dependencies]` into an ordered `(name, TomlSpec)` map.
/// Unlike `UniquePackageMap` (which materializes `PixiSpec`), this keeps the
/// flat `TomlSpec` form so member overrides can be layered without a round
/// trip back through `into_spec`.
#[derive(Debug, Default, Clone)]
pub struct WorkspaceDependencyMap {
    pub specs: IndexMap<PackageName, TomlSpec>,
}

impl<'de> toml_span::Deserialize<'de> for WorkspaceDependencyMap {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let table = match value.take() {
            ValueInner::Table(table) => table,
            inner => {
                return Err(toml_span::de_helpers::expected("a table", inner, value.span).into());
            }
        };

        let mut errors = DeserError { errors: vec![] };
        let mut specs: IndexMap<PackageName, TomlSpec> = IndexMap::new();
        let mut seen: IndexMap<PackageName, Span> = IndexMap::new();
        for (key, mut entry) in table {
            let name = match PackageName::from_str(&key.name) {
                Ok(name) => {
                    if let Some(first) = seen.get(&name) {
                        errors.errors.push(toml_span::Error {
                            kind: toml_span::ErrorKind::DuplicateKey {
                                key: key.name.into_owned(),
                                first: *first,
                            },
                            span: key.span,
                            line_info: None,
                        });
                        continue;
                    }
                    seen.insert(name.clone(), key.span);
                    name
                }
                Err(e) => {
                    errors.errors.push(toml_span::Error {
                        kind: toml_span::ErrorKind::Custom(e.to_string().into()),
                        span: key.span,
                        line_info: None,
                    });
                    continue;
                }
            };
            match TomlSpec::deserialize_from_value(&mut entry) {
                Ok(spec) => {
                    specs.insert(name, spec);
                }
                Err(e) => errors.merge(e),
            }
        }
        if errors.errors.is_empty() {
            Ok(Self { specs })
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone)]
pub struct TomlWorkspaceTarget {
    build_variants: Option<HashMap<String, Vec<String>>>,
}

/// The TOML representation of the `[[workspace]]` section in a pixi manifest.
#[derive(Debug, Clone)]
pub struct TomlWorkspace {
    // In TOML the workspace name can be empty. It is a required field though, but this is enforced
    // when converting the TOML model to the actual manifest. When using a PyProject we want to use
    // the name from the PyProject file.
    pub name: Option<String>,
    pub version: Option<Version>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub channels: IndexSet<PrioritizedChannel>,
    pub channel_priority: Option<ChannelPriority>,
    pub solve_strategy: Option<SolveStrategy>,
    pub platforms: Spanned<IndexSet<PixiPlatform>>,
    pub license: Option<Spanned<String>>,
    pub license_file: Option<Spanned<PathBuf>>,
    pub readme: Option<Spanned<PathBuf>>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
    pub conda_pypi_map: Option<CondaPypiMap>,
    pub pypi_options: Option<PypiOptions>,
    pub s3_options: Option<HashMap<String, S3Options>>,
    pub preview: TomlPreview,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlWorkspaceTarget>,
    pub build_variants: Option<HashMap<String, Vec<String>>>,
    pub build_variant_files: Option<Vec<Spanned<TomlFromStr<PathBuf>>>>,
    pub requires_pixi: Option<VersionSpec>,
    pub exclude_newer: Option<ExcludeNewer>,

    /// `[workspace.dependencies]` pool for `{ workspace = true }` inheritance.
    pub dependencies: Option<PixiSpanned<WorkspaceDependencyMap>>,

    pub span: Span,
}

impl TomlWorkspace {
    /// Converts the TOML representation of the workspace section to the actual
    /// workspace.
    ///
    /// The `root_directory` is used to resolve relative paths, if it is `None`,
    /// paths are not checked.
    pub fn into_workspace(
        self,
        external: ExternalWorkspaceProperties,
        root_directory: &Path,
    ) -> Result<WithWarnings<Workspace>, TomlError> {
        if let Some(Spanned {
            value: license,
            span,
        }) = &self.license
            && let Err(e) = spdx::Expression::parse(license)
        {
            return Err(
                GenericError::new("'license' is not a valid SPDX expression")
                    .with_span((*span).into())
                    .with_span_label(e.to_string())
                    .into(),
            );
        }

        let check_file_existence = |path: &Option<Spanned<PathBuf>>| {
            if !root_directory.as_os_str().is_empty()
                && let Some(Spanned { span, value: path }) = path
            {
                let full_path = root_directory.join(path);
                if !full_path.is_file() {
                    return Err(TomlError::from(
                        GenericError::new(format!(
                            "'{}' does not exist",
                            dunce::simplified(&full_path).display()
                        ))
                        .with_span((*span).into()),
                    ));
                }
            }
            Ok(())
        };

        check_file_existence(&self.license_file)?;
        check_file_existence(&self.readme)?;

        let WithWarnings {
            warnings: preview_warnings,
            value: preview,
        } = self.preview.into_preview();

        let mut warnings = preview_warnings;

        // An empty `conda-pypi-map = {}` is a soft-deprecated alias for
        // `conda-pypi-map = false`.
        if let Some(CondaPypiMap::Map(map)) = &self.conda_pypi_map
            && map.is_empty()
        {
            warnings.push(
                GenericError::new("`conda-pypi-map = {}` is deprecated")
                    .with_help(
                        "To disable the conda-pypi mapping, write `conda-pypi-map = false` \
                         instead.",
                    )
                    .into(),
            );
        }

        let build_variant_files_default =
            convert_build_variant_files(self.build_variant_files, root_directory)?;

        // Source specs gated on pixi-build. Path specs are left
        // workspace-relative; members re-base them at inheritance time.
        let dependencies = if let Some(deps) = self.dependencies {
            let pixi_build_enabled = preview.is_enabled(KnownPreviewFeature::PixiBuild);
            let specs = deps.value.specs;
            if !pixi_build_enabled
                && let Some((name, _)) = specs.iter().find(|(_, s)| toml_spec_is_source(s))
            {
                return Err(GenericError::new(
                    "conda source dependencies are not allowed without enabling the 'pixi-build' preview feature",
                )
                .with_help(
                    "Add `preview = [\"pixi-build\"]` to the `workspace` table of your manifest",
                )
                .with_span_label(format!("source dependency `{}`", name.as_source()))
                .with_opt_span(deps.span.clone())
                .into());
            }
            specs
        } else {
            IndexMap::new()
        };

        Ok(WithWarnings::from(Workspace {
            name: self.name.or(external.name),
            version: self.version.or(external.version),
            description: self.description.or(external.description),
            authors: self.authors.or(external.authors),
            license: self.license.map(Spanned::take).or(external.license),
            license_file: self
                .license_file
                .map(Spanned::take)
                .or(external.license_file),
            readme: self.readme.map(Spanned::take).or(external.readme),
            homepage: self.homepage.or(external.homepage),
            repository: self.repository.or(external.repository),
            documentation: self.documentation.or(external.documentation),
            channels: self.channels,
            channel_priority: self.channel_priority,
            solve_strategy: self.solve_strategy,
            platforms: self.platforms.value,
            conda_pypi_map: self.conda_pypi_map,
            pypi_options: self.pypi_options,
            s3_options: self.s3_options,
            preview,
            build_variant_files: build_variant_files_default,
            build_variants: Targets::from_default_and_user_defined(
                self.build_variants,
                self.target
                    .clone()
                    .into_iter()
                    .map(|(k, v)| (k, v.build_variants))
                    .collect(),
            ),
            requires_pixi: self.requires_pixi,
            exclude_newer: self.exclude_newer,
            exclude_newer_package_overrides: IndexMap::default(),
            pypi_exclude_newer_package_overrides: IndexMap::default(),
            dependencies,
            root_directory: root_directory.to_path_buf(),
            must_migrate: false,
        })
        .with_warnings(warnings))
    }
}

/// Returns true when the spec carries a source-style location (`path` or
/// `git`). Used to gate workspace dep entries on the `pixi-build` preview.
fn toml_spec_is_source(spec: &TomlSpec) -> bool {
    spec.location
        .as_ref()
        .is_some_and(|loc| loc.path.is_some() || loc.git.is_some())
}

fn convert_build_variant_files(
    entries: Option<Vec<Spanned<TomlFromStr<PathBuf>>>>,
    root_directory: &Path,
) -> Result<Vec<BuildVariantSource>, TomlError> {
    if let Some(entries) = entries {
        entries
            .into_iter()
            .map(|Spanned { value, span }| {
                let path = value.into_inner();
                let span_range = if span.is_empty() {
                    None
                } else {
                    Some(span.into())
                };

                if !root_directory.as_os_str().is_empty() {
                    let full_path = root_directory.join(&path);
                    if !full_path.is_file() {
                        return Err(TomlError::from(
                            GenericError::new(format!(
                                "'{}' does not exist",
                                dunce::simplified(&full_path).display()
                            ))
                            .with_opt_span(span_range),
                        ));
                    }
                }

                Ok(BuildVariantSource::File(path))
            })
            .collect()
    } else {
        Ok(Vec::new())
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlWorkspace {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let name = th.optional("name");
        let version = th
            .optional::<TomlFromStr<_>>("version")
            .map(TomlFromStr::into_inner);
        let description = th.optional("description");
        let authors = th.optional("authors");
        let channels = th
            .required::<TomlIndexSet<_>>("channels")
            .map(TomlIndexSet::into_inner)?;
        let channel_priority = th.optional("channel-priority");
        let solve_strategy = th
            .optional::<TomlWith<_, TomlFromStr<_>>>("solve-strategy")
            .map(TomlWith::into_inner);
        // Reject repeated names: `PixiPlatform`'s `Eq`/`Hash` are by name only,
        // so duplicates would otherwise silently collapse to the first entry.
        let platforms = match th.optional::<Spanned<Vec<Spanned<TomlPixiPlatform>>>>("platforms") {
            None => None,
            Some(spanned) => {
                let span = spanned.span;
                let mut value = IndexSet::new();
                let mut seen: IndexMap<String, Span> = IndexMap::new();
                for entry in spanned.value {
                    let entry_span = entry.span;
                    let platform = entry.value.into_inner();
                    let name = platform.name().to_string();
                    if let Some(first) = seen.get(&name) {
                        return Err(toml_span::Error {
                            kind: toml_span::ErrorKind::DuplicateKey {
                                key: name,
                                first: *first,
                            },
                            span: entry_span,
                            line_info: None,
                        }
                        .into());
                    }
                    seen.insert(name, entry_span);
                    value.insert(platform);
                }
                Some(Spanned { span, value })
            }
        };
        let license = th.optional("license");
        let license_file = th
            .optional::<TomlWith<_, Spanned<TomlFromStr<_>>>>("license-file")
            .map(TomlWith::into_inner);
        let readme = th
            .optional::<TomlWith<_, Spanned<TomlFromStr<_>>>>("readme")
            .map(TomlWith::into_inner);
        let homepage = th
            .optional::<TomlFromStr<_>>("homepage")
            .map(TomlFromStr::into_inner);
        let repository = th
            .optional::<TomlFromStr<_>>("repository")
            .map(TomlFromStr::into_inner);
        let documentation = th
            .optional::<TomlFromStr<_>>("documentation")
            .map(TomlFromStr::into_inner);
        let conda_pypi_map = th.optional("conda-pypi-map");
        let pypi_options = th.optional("pypi-options");
        let s3_options = th
            .optional::<TomlHashMap<_, _>>("s3-options")
            .map(TomlHashMap::into_inner);
        let preview = th.optional("preview").unwrap_or_default();
        let target = th
            .optional::<TomlIndexMap<_, _>>("target")
            .map(TomlIndexMap::into_inner);
        let build_variant_files =
            th.optional::<Vec<Spanned<TomlFromStr<PathBuf>>>>("build-variants-files");
        let build_variants = th
            .optional::<TomlHashMap<_, _>>("build-variants")
            .map(TomlHashMap::into_inner);
        let requires_pixi = th
            .optional::<TomlVersionSpecStr>("requires-pixi")
            .map(TomlVersionSpecStr::into_inner);
        let exclude_newer = th
            .optional::<TomlWith<_, TomlFromStr<_>>>("exclude-newer")
            .map(TomlWith::into_inner);
        let dependencies = th.optional("dependencies");

        th.finalize(None)?;

        Ok(TomlWorkspace {
            name,
            version,
            description,
            authors,
            channels,
            channel_priority,
            solve_strategy,
            platforms: platforms.unwrap_or_default(),
            license,
            license_file,
            readme,
            homepage,
            repository,
            documentation,
            conda_pypi_map,
            pypi_options,
            s3_options,
            preview,
            target: target.unwrap_or_default(),
            build_variants,
            build_variant_files,
            requires_pixi,
            exclude_newer,
            dependencies,
            span: value.span,
        })
    }
}

impl<'de> toml_span::Deserialize<'de> for TomlWorkspaceTarget {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;

        let build_variants = th
            .optional::<TomlHashMap<_, _>>("build-variants")
            .map(TomlHashMap::into_inner);

        th.finalize(None)?;

        Ok(TomlWorkspaceTarget { build_variants })
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use crate::{
        toml::{FromTomlStr, TomlWorkspace, manifest::ExternalWorkspaceProperties},
        utils::test_utils::expect_parse_failure,
    };
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;

    #[test]
    fn test_invalid_license() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = []
        license = "MIT OR FOOBAR"
        "#,
        ));
    }

    #[test]
    fn test_invalid_license_file() {
        let input = r#"
        channels = []
        platforms = []
        license-file = "LICENSE.txt"
        "#;
        let path = Path::new("/nonexistent");
        let parse_error = TomlWorkspace::from_toml_str(input)
            .and_then(|w| w.into_workspace(ExternalWorkspaceProperties::default(), path))
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r###"
         × '/nonexistent/LICENSE.txt' does not exist
          ╭─[pixi.toml:4:25]
        3 │         platforms = []
        4 │         license-file = "LICENSE.txt"
          ·                         ───────────
        5 │
          ╰────
        "###);
    }

    #[test]
    fn test_invalid_readme() {
        let input = r#"
        channels = []
        platforms = []
        readme = "README.md"
        "#;
        let path = Path::new("/nonexistent");
        let parse_error = TomlWorkspace::from_toml_str(input)
            .and_then(|w| w.into_workspace(ExternalWorkspaceProperties::default(), path))
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r###"
         × '/nonexistent/README.md' does not exist
          ╭─[pixi.toml:4:19]
        3 │         platforms = []
        4 │         readme = "README.md"
          ·                   ─────────
        5 │
          ╰────
        "###);
    }

    #[test]
    fn test_missing_build_variant_file() {
        let input = r#"
        channels = []
        platforms = []
        build-variants-files = ["missing.yaml"]
        "#;
        let path = Path::new("/nonexistent");
        let parse_error = TomlWorkspace::from_toml_str(input)
            .and_then(|w| w.into_workspace(ExternalWorkspaceProperties::default(), path))
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r#"
         × '/nonexistent/missing.yaml' does not exist
          ╭─[pixi.toml:4:34]
        3 │         platforms = []
        4 │         build-variants-files = ["missing.yaml"]
          ·                                  ────────────
        5 │
          ╰────
        "#);
    }

    #[test]
    fn test_workspace_platforms_mixed_string_and_table() {
        let input = r#"
        channels = []
        platforms = [
          "linux-64",
          { name = "linux-64-cuda", platform = "linux-64", cuda = "12.0" },
          { name = "osx-arm64" },
        ]
        "#;
        let workspace = TomlWorkspace::from_toml_str(input)
            .unwrap()
            .into_workspace(ExternalWorkspaceProperties::default(), Path::new(""))
            .unwrap()
            .value;

        let names: Vec<&str> = workspace
            .platforms
            .iter()
            .map(|wp| wp.name().as_str())
            .collect();
        assert_eq!(names, vec!["linux-64", "linux-64-cuda", "osx-arm64"]);

        let cuda_name = crate::PixiPlatformName::try_from("linux-64-cuda").unwrap();
        let cuda = workspace.platform_by_name(&cuda_name).unwrap();
        assert_eq!(
            cuda.subdir(),
            rattler_conda_types::Platform::Linux64,
            "custom name should keep linux-64 as its subdir"
        );
        let declared: Vec<String> = cuda
            .declared_virtual_packages()
            .iter()
            .map(|vp| vp.to_string())
            .collect();
        assert_eq!(
            declared,
            vec![
                "__cuda=12.0".to_string(),
                "__unix=0=0".to_string(),
                "__linux=4.18".to_string(),
                "__glibc=2.28".to_string(),
                "__archspec=0=x86_64".to_string(),
            ],
            "rich platforms materialise the subdir defaults alongside the declared __cuda; \
             `__unix=0=0` reflects rattler's version=0, build_string=\"0\" shape",
        );

        // Subdir entries (`linux-64`, `osx-arm64` with name == subdir)
        // are still recognisable by `is_subdir_platform`. Their declared
        // virtual-package list is the subdir defaults rather than empty.
        let subdirs: Vec<_> = workspace
            .platforms
            .iter()
            .filter(|wp| wp.is_subdir_platform())
            .map(|wp| wp.subdir())
            .collect();
        assert_eq!(
            subdirs,
            vec![
                rattler_conda_types::Platform::Linux64,
                rattler_conda_types::Platform::OsxArm64,
            ]
        );
    }

    /// Two platform entries that resolve to the same name must be rejected,
    /// not silently collapsed to the first (`PixiPlatform` is keyed by name).
    #[test]
    fn test_duplicate_workspace_platform_name_rejected() {
        assert_snapshot!(expect_parse_failure(
            r#"
        [workspace]
        channels = []
        platforms = [
          { name = "gpu", platform = "linux-64", cuda = "12.0" },
          { name = "gpu", platform = "linux-64", cuda = "13.0" },
        ]
        "#,
        ));
    }

    #[test]
    fn test_invalid_exclude_newer() {
        let input = r#"
        channels = []
        platforms = []
        exclude-newer = "date"
        "#;
        let path = Path::new("");
        let parse_error = TomlWorkspace::from_toml_str(input)
            .and_then(|w| w.into_workspace(ExternalWorkspaceProperties::default(), path))
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r#"
         × `date` is neither a valid duration, date (input contains invalid characters), nor timestamp (premature end of input)
          ╭─[pixi.toml:4:26]
        3 │         platforms = []
        4 │         exclude-newer = "date"
          ·                          ────
        5 │
          ╰────
        "#);
    }
}
