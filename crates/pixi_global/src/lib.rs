pub mod common;
#[cfg(unix)] // Completions are only supported on unix-like systems
pub mod completions;
pub mod install;
pub mod list;
pub mod project;
pub mod trampoline;

pub use common::{BinDir, EnvChanges, EnvDir, EnvRoot, EnvState, StateChange, StateChanges};
pub use project::{EnvironmentName, ExposedName, Mapping, Project};

use pixi_utils::prefix::{Executable, Prefix};
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

/// Processes prefix records (that you can get by using `find_installed_packages`)
/// to filter and collect executable files.
///
/// This also includes symlinks to executables that may not be tracked in prefix records
/// (e.g., npm-installed packages).
pub fn find_executables_for_many_records(
    prefix: &Prefix,
    prefix_packages: &[PrefixRecord],
) -> Vec<Executable> {
    prefix.find_executables(prefix_packages)
}

fn is_executable(prefix: &Prefix, relative_path: &Path) -> bool {
    // Check if the file is executable
    let absolute_path = prefix.root().join(relative_path);
    is_executable::is_executable(absolute_path)
}
