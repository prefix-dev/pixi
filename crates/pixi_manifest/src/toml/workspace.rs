use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use indexmap::{IndexMap, IndexSet};
use pixi_toml::{TomlFromStr, TomlHashMap, TomlIndexMap, TomlIndexSet, TomlWith};
use rattler_conda_types::{NamedChannelOrUrl, Platform, Version};
use toml_span::{de_helpers::TableHelper, DeserError, Error, ErrorKind, Span, Spanned, Value};
use url::Url;

use crate::toml::manifest::ExternalWorkspaceProperties;
use crate::{
    error::GenericError,
    pypi::pypi_options::PypiOptions,
    toml::{platform::TomlPlatform, preview::TomlPreview},
    utils::PixiSpanned,
    workspace::ChannelPriority,
    PrioritizedChannel, S3Options, TargetSelector, Targets, TomlError, WithWarnings, Workspace,
};

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
    pub platforms: Spanned<IndexSet<Platform>>,
    pub license: Option<Spanned<String>>,
    pub license_file: Option<Spanned<PathBuf>>,
    pub readme: Option<Spanned<PathBuf>>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
    pub conda_pypi_map: Option<HashMap<NamedChannelOrUrl, String>>,
    pub pypi_options: Option<PypiOptions>,
    pub s3_options: Option<HashMap<String, S3Options>>,
    pub preview: TomlPreview,
    pub target: IndexMap<PixiSpanned<TargetSelector>, TomlWorkspaceTarget>,
    pub build_variants: Option<HashMap<String, Vec<String>>>,

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
        root_directory: Option<&Path>,
    ) -> Result<WithWarnings<Workspace>, TomlError> {
        if let Some(Spanned {
            value: license,
            span,
        }) = &self.license
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

        let check_file_existence = |path: &Option<Spanned<PathBuf>>| {
            if let (Some(root_directory), Some(Spanned { span, value: path })) =
                (root_directory, path)
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

        let warnings = preview_warnings;

        Ok(WithWarnings::from(Workspace {
            name: self.name.or(external.name).ok_or(Error {
                kind: ErrorKind::MissingField("name"),
                span: self.span,
                line_info: None,
            })?,
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
            platforms: self.platforms.value,
            conda_pypi_map: self.conda_pypi_map,
            pypi_options: self.pypi_options,
            s3_options: self.s3_options,
            preview,
            build_variants: Targets::from_default_and_user_defined(
                self.build_variants,
                self.target
                    .clone()
                    .into_iter()
                    .map(|(k, v)| (k, v.build_variants))
                    .collect(),
            ),
        })
        .with_warnings(warnings))
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
        let platforms = th
            .optional::<TomlWith<_, Spanned<TomlIndexSet<TomlPlatform>>>>("platforms")
            .map(TomlWith::into_inner);
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
        let conda_pypi_map = th
            .optional::<TomlHashMap<_, _>>("conda-pypi-map")
            .map(TomlHashMap::into_inner);
        let pypi_options = th.optional("pypi-options");
        let s3_options = th
            .optional::<TomlHashMap<_, _>>("s3-options")
            .map(TomlHashMap::into_inner);
        let preview = th.optional("preview").unwrap_or_default();
        let target = th
            .optional::<TomlIndexMap<_, _>>("target")
            .map(TomlIndexMap::into_inner);
        let build_variants = th
            .optional::<TomlHashMap<_, _>>("build-variants")
            .map(TomlHashMap::into_inner);

        th.finalize(None)?;

        Ok(TomlWorkspace {
            name,
            version,
            description,
            authors,
            channels,
            channel_priority,
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

    use insta::assert_snapshot;

    use crate::toml::manifest::ExternalWorkspaceProperties;
    use crate::{
        toml::{FromTomlStr, TomlWorkspace},
        utils::test_utils::{expect_parse_failure, format_parse_error},
    };

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
        let path = Path::new("");
        let parse_error = TomlWorkspace::from_toml_str(input)
            .and_then(|w| w.into_workspace(ExternalWorkspaceProperties::default(), Some(path)))
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r###"
         × 'LICENSE.txt' does not exist
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
        let path = Path::new("");
        let parse_error = TomlWorkspace::from_toml_str(input)
            .and_then(|w| w.into_workspace(ExternalWorkspaceProperties::default(), Some(path)))
            .unwrap_err();
        assert_snapshot!(format_parse_error(input, parse_error), @r###"
         × 'README.md' does not exist
          ╭─[pixi.toml:4:19]
        3 │         platforms = []
        4 │         readme = "README.md"
          ·                   ─────────
        5 │
          ╰────
        "###);
    }
}
