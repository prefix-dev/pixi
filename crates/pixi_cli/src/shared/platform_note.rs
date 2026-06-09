//! Shared one-line note naming the platform an environment was installed for,
//! used by `list` and `tree` to label which prefix their output reflects.

use pixi_core::workspace::Environment;
use rattler_conda_types::Platform;

/// The platform `environment` was installed for, read from its
/// `conda-meta/pixi` marker file, with an emulation hint when that subdir
/// differs from the host. `None` when the environment isn't installed, so
/// callers print nothing.
pub(crate) fn installed_platform_note(environment: &Environment<'_>) -> Option<String> {
    let (resolved, _minimum) = environment.installed_platforms();
    let subdir = resolved?.subdir();
    let host = Platform::current();
    if subdir == host {
        Some(subdir.to_string())
    } else {
        Some(format!("{subdir} (emulated on {host})"))
    }
}
