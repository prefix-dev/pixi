use rattler_conda_types::{MatchSpec, NoArchType, PackageName, Platform, VersionWithSource};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CondaPackageMetadata {
    /// The name of the package.
    pub name: PackageName,

    /// The version of the package.
    pub version: VersionWithSource,

    /// The build hash of the package.
    pub build: String,

    /// The build number of the package.
    pub build_number: u64,

    /// The subdir or platform
    pub subdir: Platform,

    /// The dependencies of the package
    #[serde(default)]
    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub depends: Vec<MatchSpec>,

    /// The constrains of the package
    #[serde(default)]
    #[serde_as(as = "Vec<DisplayFromStr>")]
    pub constraints: Vec<MatchSpec>,

    /// The license of the package
    pub license: Option<String>,

    /// The license family of the package
    pub license_family: Option<String>,

    /// The noarch type of the package
    pub noarch: NoArchType,
}
