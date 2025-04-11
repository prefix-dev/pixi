use std::collections::HashMap;

use rattler_conda_types::{NoArchType, PackageName, Platform, VersionWithSource};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::SourcePackageSpecV1;

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
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
    pub depends: Vec<String>,

    /// The constrains of the package
    #[serde(default)]
    pub constraints: Vec<String>,

    /// The license of the package
    pub license: Option<String>,

    /// The license family of the package
    pub license_family: Option<String>,

    /// The noarch type of the package
    pub noarch: NoArchType,

    /// Describes any packages that should be built from a particular
    /// source.
    #[serde(default)]
    pub sources: HashMap<String, SourcePackageSpecV1>,
}
