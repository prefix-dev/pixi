use pixi_core::lock_file::{ReinstallEnvironment, ReinstallPackages};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ReinstallOptions {
    /// Specifies the packages that should be reinstalled.
    pub reinstall_packages: ReinstallPackages,

    /// Specifies the environments that should be reinstalled.
    pub reinstall_environments: ReinstallEnvironment,
}
