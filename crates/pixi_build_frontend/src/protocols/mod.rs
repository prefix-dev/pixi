//! Implementations of the [`crate::Protocol`] type for various backends.

use std::path::{Path, PathBuf};

use pixi_consts::consts;
pub(super) mod conda_build;
mod error;
pub(super) mod pixi;
pub(super) mod rattler_build;

/// Try to find a pixi manifest in the given source directory.
pub fn find_pixi_manifest(source_dir: &Path) -> Option<PathBuf> {
    let pixi_manifest_path = source_dir.join(consts::PROJECT_MANIFEST);
    if pixi_manifest_path.exists() {
        return Some(pixi_manifest_path);
    }

    let pyproject_manifest_path = source_dir.join(consts::PYPROJECT_MANIFEST);
    // TODO: Really check if this is a pixi project.
    if pyproject_manifest_path.is_file() {
        return Some(pyproject_manifest_path);
    }

    None
}
