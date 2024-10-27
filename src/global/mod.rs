pub(crate) mod common;
pub(crate) mod install;
pub(crate) mod list;
pub(crate) mod project;
pub(crate) mod trampoline;

pub(crate) use common::{BinDir, EnvChanges, EnvDir, EnvRoot, EnvState, StateChange, StateChanges};
pub(crate) use install::extract_executable_from_script;
use pixi_utils::strip_executable_extension;
pub(crate) use project::{EnvironmentName, ExposedName, Mapping, Project};

use crate::prefix::{Executable, Prefix};
use rattler_conda_types::PrefixRecord;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf}
};

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

/// Processes prefix records (that you can get by using `find_installed_packages`)
/// to filter and collect executable files.
pub fn find_executables_for_many_records(prefix: &Prefix, prefix_packages: &[PrefixRecord]) -> Vec<Executable> {
    let executables = prefix_packages
        .iter()
        .flat_map(|record| {
            record
                .files
                .iter()
                .filter(|relative_path| is_executable(prefix, relative_path))
                .filter_map(|path| {
                    path.iter().last().and_then(OsStr::to_str).map(|name| {
                        Executable::new(
                            strip_executable_extension(name.to_string()),
                            path.clone(),
                        )
                    })
                })
        })
        .collect();
    executables
}

fn is_executable(prefix: &Prefix, relative_path: &Path) -> bool {
    // Check if the file is executable
    let absolute_path = prefix.root().join(relative_path);
    is_executable::is_executable(absolute_path)
}
