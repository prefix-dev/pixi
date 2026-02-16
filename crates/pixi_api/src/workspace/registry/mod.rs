use std::path::PathBuf;

use pixi_consts::consts;
use pixi_config::pixi_home;


/// Returns the path to the workspace registry file
pub fn workspace_registry_path() -> Option<PathBuf> {
    pixi_home().map(|d| d.join(consts::WORKSPACES_REGISTRY))
}
