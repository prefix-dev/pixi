mod common;
mod expose;
mod install;
mod project;

pub(crate) use expose::{expose_add, expose_remove};

pub(crate) use common::{
    channel_name_from_prefix, find_designated_package, BinDir, EnvDir, EnvRoot,
};
pub(crate) use install::{create_executable_scripts, script_exec_mapping, sync};
pub(crate) use project::{EnvironmentName, ExposedKey, Project, MANIFEST_DEFAULT_NAME};

use crate::prefix::Prefix;
pub(crate) use common::{BinDir, EnvDir, EnvRoot};
pub(crate) use install::sync;
pub(crate) use project::{EnvironmentName, ExposedKey, Project};
use rattler_conda_types::PrefixRecord;
use std::path::{Path, PathBuf};

/// Find the executable scripts within the specified package installed in this
/// conda prefix.
fn find_executables(prefix: &Prefix, prefix_package: &PrefixRecord) -> Vec<PathBuf> {
    prefix_package
        .files
        .iter()
        .filter(|&relative_path| is_executable(prefix, relative_path))
        .cloned()
        .collect()
}

fn is_executable(prefix: &Prefix, relative_path: &Path) -> bool {
    // Check if the file is in a known executable directory.
    let binary_folders = if cfg!(windows) {
        &([
            "",
            "Library/mingw-w64/bin/",
            "Library/usr/bin/",
            "Library/bin/",
            "Scripts/",
            "bin/",
        ][..])
    } else {
        &(["bin"][..])
    };

    let parent_folder = match relative_path.parent() {
        Some(dir) => dir,
        None => return false,
    };

    if !binary_folders
        .iter()
        .any(|bin_path| Path::new(bin_path) == parent_folder)
    {
        return false;
    }

    // Check if the file is executable
    let absolute_path = prefix.root().join(relative_path);
    is_executable::is_executable(absolute_path)
}
