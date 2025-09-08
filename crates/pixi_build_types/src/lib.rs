#[deny(missing_docs)]
mod capabilities;
mod channel_configuration;
mod conda_package_metadata;
pub mod procedures;
mod project_model;

use std::fmt::Display;
use std::sync::LazyLock;

pub use capabilities::{BackendCapabilities, FrontendCapabilities};
pub use channel_configuration::ChannelConfiguration;
pub use conda_package_metadata::CondaPackageMetadata;
pub use project_model::{
    BinaryPackageSpecV1, GitReferenceV1, GitSpecV1, NamedSpecV1, PackageSpecV1, PathSpecV1,
    ProjectModelV1, SourcePackageName, SourcePackageSpecV1, TargetSelectorV1, TargetV1, TargetsV1,
    UrlSpecV1, VersionedProjectModel,
};
use rattler_conda_types::{
    GenericVirtualPackage, PackageName, Platform, Version, VersionSpec,
    version_spec::{LogicalOperator, RangeOperator},
};
use serde::{Deserialize, Serialize};

// Version 0: Initial version
// Version 1: Added conda/outputs and conda/build_v1
// Version 2: Name in project models can be `None`.

/// The constraint for the pixi build api version package
/// Adding this constraint when solving a pixi build backend environment ensures
/// that a backend is selected that uses the same interface version as Pixi does
pub static PIXI_BUILD_API_VERSION_NAME: LazyLock<PackageName> =
    LazyLock::new(|| PackageName::new_unchecked("pixi-build-api-version"));
pub const PIXI_BUILD_API_VERSION_LOWER: u64 = 0;
pub const PIXI_BUILD_API_VERSION_CURRENT: u64 = 2;
pub const PIXI_BUILD_API_VERSION_UPPER: u64 = PIXI_BUILD_API_VERSION_CURRENT + 1;
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

/// A type that represents the version of the Pixi Build API.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct PixiBuildApiVersion(pub u64);

impl PixiBuildApiVersion {
    /// Constructs this type from a `Version` object.
    pub fn from_version(version: &Version) -> Option<Self> {
        let first_segment = version.segments().next()?;
        if first_segment.component_count() == 1 {
            first_segment
                .components()
                .next()
                .and_then(|c| c.as_number())
                .map(PixiBuildApiVersion)
        } else {
            None
        }
    }

    /// Returns the "current" version of the Pixi Build API.
    pub fn current() -> Self {
        PixiBuildApiVersion(PIXI_BUILD_API_VERSION_CURRENT)
    }

    /// Returns the backend capabilities that are expected for this version.
    pub fn expected_backend_capabilities(&self) -> BackendCapabilities {
        match self.0 {
            0 => BackendCapabilities {
                provides_conda_metadata: Some(true),
                provides_conda_build: Some(true),
                highest_supported_project_model: Some(1),
                ..BackendCapabilities::default()
            },
            1 => BackendCapabilities {
                provides_conda_outputs: Some(true),
                provides_conda_build_v1: Some(true),
                ..Self(0).expected_backend_capabilities()
            },
            2 => BackendCapabilities {
                ..Self(1).expected_backend_capabilities()
            },
            _ => BackendCapabilities::default(),
        }
    }

    /// Returns true if this version of the protocol supports the name field in the project model to be `None`.
    pub fn supports_name_none(&self) -> bool {
        self.0 >= 2
    }
}

impl Display for PixiBuildApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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
