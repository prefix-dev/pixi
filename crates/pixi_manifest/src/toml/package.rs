use std::path::PathBuf;

use indexmap::IndexMap;
pub use pixi_toml::TomlFromStr;
use pixi_toml::{Same, TomlIndexMap, TomlWith};
use rattler_conda_types::Version;
use thiserror::Error;
use toml_span::{de_helpers::TableHelper, DeserError, Error, ErrorKind, Span, Value};
use url::Url;

use crate::{
    package::Package,
    toml::{
        package_target::TomlPackageTarget, workspace::ExternalWorkspaceProperties, TomlPackageBuild,
    },
    utils::{package_map::UniquePackageMap, PixiSpanned},
    PackageManifest, TargetSelector, Targets, TomlError, WorkspaceManifest,
};

/// The TOML representation of the `[package]` section in a pixi manifest.
///
/// In TOML some of the fields can be empty even though they are required in the
/// data model (e.g. `name`, `version`). This is allowed because some of the
/// fields might be derived from other sections of the TOML.
#[derive(Debug)]
pub struct TomlPackage {
    // In TOML the workspace name can be empty. It is a required field though, but this is enforced
    // when converting the TOML model to the actual manifest. When using a PyProject we want to use
    // the name from the PyProject file.
    pub name: Option<String>,
    pub version: Option<Version>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub license_file: Option<PathBuf>,
    pub readme: Option<PathBuf>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
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
            .optional::<TomlFromStr<Version>>("version")
            .map(TomlFromStr::into_inner);
        let description = th.optional("description");
        let authors = th.optional("authors");
        let license = th.optional("license");
        let license_file = th
            .optional::<TomlFromStr<_>>("license-file")
            .map(TomlFromStr::into_inner);
        let readme = th
            .optional::<TomlFromStr<_>>("readme")
            .map(TomlFromStr::into_inner);
        let homepage = th
            .optional::<TomlFromStr<_>>("homepage")
            .map(TomlFromStr::into_inner);
        let repository = th
            .optional::<TomlFromStr<_>>("repository")
            .map(TomlFromStr::into_inner);
        let documentation = th
            .optional::<TomlFromStr<_>>("documentation")
            .map(TomlFromStr::into_inner);
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

/// Defines some of the properties that might be defined in other parts of the
/// manifest but we do require to be set in the package section.
///
/// This can be used to inject these properties.
#[derive(Debug, Clone, Default)]
pub struct ExternalPackageProperties {
    pub name: Option<String>,
    pub version: Option<Version>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub license_file: Option<PathBuf>,
    pub readme: Option<PathBuf>,
    pub homepage: Option<Url>,
    pub repository: Option<Url>,
    pub documentation: Option<Url>,
}

impl From<ExternalWorkspaceProperties> for ExternalPackageProperties {
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
    pub fn into_manifest(
        self,
        external: ExternalPackageProperties,
        workspace_manifest: &WorkspaceManifest,
    ) -> Result<PackageManifest, TomlError> {
        let name = self.name.or(external.name).ok_or(Error {
            kind: ErrorKind::MissingField("name"),
            span: self.span,
            line_info: None,
        })?;
        let version = self.version.or(external.version).ok_or(Error {
            kind: ErrorKind::MissingField("version"),
            span: self.span,
            line_info: None,
        })?;

        let default_package_target = TomlPackageTarget {
            run_dependencies: self.run_dependencies,
            host_dependencies: self.host_dependencies,
            build_dependencies: self.build_dependencies,
        }
        .into_package_target(&workspace_manifest.workspace.preview)?;

        let targets = self
            .target
            .into_iter()
            .map(|(selector, target)| {
                let target = target.into_package_target(&workspace_manifest.workspace.preview)?;
                Ok::<_, TomlError>((selector, target))
            })
            .collect::<Result<_, _>>()?;

        Ok(PackageManifest {
            package: Package {
                name,
                version,
                description: self.description.or(external.description),
                authors: self.authors.or(external.authors),
                license: self.license.or(external.license),
                license_file: self.license_file.or(external.license_file),
                readme: self.readme.or(external.readme),
                homepage: self.homepage.or(external.homepage),
                repository: self.repository.or(external.repository),
                documentation: self.documentation.or(external.documentation),
            },
            build: self.build.into_build_system()?,
            targets: Targets::from_default_and_user_defined(default_package_target, targets),
        })
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
}
