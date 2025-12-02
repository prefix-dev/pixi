use pixi_core::environment::LockFileUsage;
use pixi_manifest::FeatureName;
use pixi_spec::GitReference;
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DependencyOptions {
    /// The feature for which the dependency should be modified.
    pub feature: FeatureName,
    /// The platform for which the dependency should be modified.
    pub platforms: Vec<Platform>,
    /// Don't modify the environment, only modify the lock-file.
    pub no_install: bool,
    pub lock_file_usage: LockFileUsage,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct GitOptions {
    pub git: Option<Url>,
    pub reference: GitReference,
    pub subdir: Option<String>,
}
