use std::path::PathBuf;

use rattler_conda_types::Version;
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};
use thiserror::Error;
use url::Url;

use crate::package::Package;
use crate::toml::workspace::ExternalWorkspaceProperties;

/// The TOML representation of the `[workspace]` section in a pixi manifest.
///
/// In TOML some of the fields can be empty even though they are required in the
/// data model (e.g. `name`, `version`). This is allowed because some of the
/// fields might be derived from other sections of the TOML.
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct TomlPackage {
    // In TOML the workspace name can be empty. It is a required field though, but this is enforced
    // when converting the TOML model to the actual manifest. When using a PyProject we want to use
    // the name from the PyProject file.
    pub name: Option<String>,
    #[serde_as(as = "Option<DisplayFromStr>")]
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

/// Defines some of the properties that might be defined in other parts of the
/// manifest but we do require to be set in the package section.
///
/// This can be used to inject these properties.
#[derive(Debug, Clone)]
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
}

impl TomlPackage {
    pub fn into_package(
        self,
        external: ExternalPackageProperties,
    ) -> Result<Package, PackageError> {
        let name = self
            .name
            .or(external.name)
            .ok_or(PackageError::MissingName)?;
        let version = self
            .version
            .or(external.version)
            .ok_or(PackageError::MissingVersion)?;

        Ok(Package {
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
        })
    }
}
