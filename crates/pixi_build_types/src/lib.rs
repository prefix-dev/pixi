#[deny(missing_docs)]
mod capabilities;
mod channel_configuration;
mod conda_package_metadata;
pub mod procedures;
mod project_model;

pub use capabilities::{BackendCapabilities, FrontendCapabilities};
pub use channel_configuration::ChannelConfiguration;
pub use conda_package_metadata::CondaPackageMetadata;
pub use project_model::{
    DependencySpecV1, GitReferenceV1, GitSpecV1, PathSpecV1, PixiSpecV1, ProjectModelV1,
    SourcePackageName, TargetSelectorV1, TargetV1, TargetsV1, UrlSpecV1, VersionedProjectModel,
};
use rattler_conda_types::{GenericVirtualPackage, Platform};
use serde::{Deserialize, Serialize};

/// A platform and associated virtual packages
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformAndVirtualPackages {
    /// The platform
    pub platform: Platform,

    /// Virtual packages associated with the platform. Or `None` if the virtual
    /// packages are not specified.
    pub virtual_packages: Option<Vec<GenericVirtualPackage>>,
}
