use pixi_core::lock_file::{ReinstallEnvironment, ReinstallPackages};
use pixi_manifest::PixiPlatformName;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ReinstallOptions {
    /// Specifies the packages that should be reinstalled.
    pub reinstall_packages: ReinstallPackages,

    /// Specifies the environments that should be reinstalled.
    pub reinstall_environments: ReinstallEnvironment,

    /// Targets a specific workspace platform instead of letting the
    /// environment pick one host-aware. Equivalent to `pixi install
    /// --platform <name>`: the host-virtual-package satisfaction check
    /// is skipped, so a target the local machine can't run still
    /// proceeds (the resulting packages just won't be executable here).
    #[serde(default)]
    pub target_platform: Option<PixiPlatformName>,
}
