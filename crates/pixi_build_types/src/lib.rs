#[deny(missing_docs)]
mod capabilities;
mod channel_configuration;
mod conda_package_metadata;
pub mod procedures;
mod project_model;

use std::sync::LazyLock;

pub use capabilities::{BackendCapabilities, FrontendCapabilities};
pub use channel_configuration::ChannelConfiguration;
pub use conda_package_metadata::CondaPackageMetadata;
pub use project_model::{
    BinaryPackageSpecV1, GitReferenceV1, GitSpecV1, PackageSpecV1, PathSpecV1, ProjectModelV1,
    SourcePackageName, SourcePackageSpecV1, TargetSelectorV1, TargetV1, TargetsV1, UrlSpecV1,
    VersionedProjectModel,
};
use rattler_conda_types::{
    GenericVirtualPackage, PackageName, Platform, Version, VersionSpec,
    version_spec::{LogicalOperator, RangeOperator},
};
use serde::{Deserialize, Serialize};

/// The constraint for the pixi build api version package
/// Adding this constraint when solving a pixi build backend environment ensures that
/// a backend is selected that uses the same interface version as Pixi does
pub static PIXI_BUILD_API_VERSION_NAME: LazyLock<PackageName> =
    LazyLock::new(|| PackageName::new_unchecked("pixi-build-api-version"));
pub const PIXI_BUILD_API_VERSION_LOWER: u64 = 0;
pub const PIXI_BUILD_API_VERSION_UPPER: u64 = 1;
pub static PIXI_BUILD_API_VERSION_SPEC: LazyLock<VersionSpec> = LazyLock::new(|| {
    VersionSpec::Group(
        LogicalOperator::And,
        Vec::from([
            VersionSpec::Range(
                RangeOperator::GreaterEquals,
                Version::major(PIXI_BUILD_API_VERSION_LOWER),
            ),
            VersionSpec::Range(
                RangeOperator::Less,
                Version::major(PIXI_BUILD_API_VERSION_UPPER),
            ),
        ]),
    )
});

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
