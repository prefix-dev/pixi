mod build_system;
mod channel;
mod document;
mod environment;
mod feature;
mod manifest;
mod package;
mod platform;
mod pypi_options;
mod pyproject;
mod system_requirements;
mod target;
mod task;
mod workspace;

pub use build_system::TomlBuildSystem;
pub use channel::TomlPrioritizedChannel;
pub use document::TomlDocument;
pub use environment::{TomlEnvironment, TomlEnvironmentList};
pub use feature::TomlFeature;
pub use manifest::TomlManifest;
pub use package::{ExternalPackageProperties, PackageError, TomlPackage};
pub use platform::TomlPlatform;
pub use target::TomlTarget;
use toml_span::DeserError;
pub use workspace::{ExternalWorkspaceProperties, TomlWorkspace, WorkspaceError};

use crate::TomlError;

pub trait FromTomlStr {
    fn from_toml_str(source: &str) -> Result<Self, TomlError>
    where
        Self: Sized;
}

impl<T: for<'de> toml_span::Deserialize<'de>> FromTomlStr for T {
    fn from_toml_str(source: &str) -> Result<Self, TomlError> {
        toml_span::parse(source)
            .map_err(DeserError::from)
            .and_then(|mut v| toml_span::Deserialize::deserialize(&mut v))
            .map_err(TomlError::from)
    }
}
