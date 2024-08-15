use rattler_conda_types::{MatchSpec, PackageName, Platform, Version};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaPackageMetadata {
    /// The name of the package.
    pub name: PackageName,

    /// The version of the package.
    pub version: Version,

    /// The build hash of the package.
    pub build: String,

    /// The build number of the package.
    pub build_number: u64,

    /// The subdir or platform
    pub subdir: Platform,

    /// The dependencies of the package
    #[serde_as(as = "Option<Vec<DisplayFromStr>>")]
    pub depends: Option<Vec<MatchSpec>>,

    /// The constrains of the package
    #[serde_as(as = "Option<Vec<DisplayFromStr>>")]
    pub constrains: Option<Vec<MatchSpec>>,

    /// The license of the package
    pub license: Option<String>,

    /// The license family of the package
    pub license_family: Option<String>,
}
