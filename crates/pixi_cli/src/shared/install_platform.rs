//! Shared `--platform` resolution for `install` and `reinstall`. Returns
//! a [`PixiPlatformName`] the caller threads down to the install path so
//! the satisfiability check uses it directly rather than running
//! host-detection and rejecting cross-targets.

use pixi_api::workspace::platforms::resolve_platforms;
use pixi_core::Workspace;
use pixi_manifest::{HasWorkspaceManifest, PixiPlatformName};
use rattler_conda_types::Platform;

/// Resolve `--platform` to a workspace platform name, emitting a warning
/// when the resolved subdir isn't a candidate for the current host.
/// Returns `Ok(None)` when the flag was unset; the caller threads the
/// result down into the install path.
pub(crate) fn resolve_install_platform(
    workspace: &Workspace,
    platform: Option<&PixiPlatformName>,
) -> miette::Result<Option<PixiPlatformName>> {
    let Some(name) = platform else {
        return Ok(None);
    };
    let workspace_platforms = workspace.workspace_manifest().workspace.platforms.clone();
    let resolved = resolve_platforms(&workspace_platforms, std::slice::from_ref(name))?
        .into_iter()
        .next()
        .expect("resolve_platforms preserves length");
    let subdir = resolved.subdir();
    let current = Platform::current();
    let candidates = workspace
        .workspace_manifest()
        .workspace
        .candidate_subdirs(current);
    if !candidates.contains(&subdir) {
        let warning = format!(
            "installing for platform '{name}' (subdir '{subdir}'), \
             which this machine ('{current}') can not run -- packages will \
             be downloaded and extracted but won't be executable here",
        );
        tracing::warn!("{warning}");
        eprintln!("{} {warning}", console::style("warning:").yellow().bold());
    }
    Ok(Some(resolved.name().clone()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn workspace_with_platforms(platforms: &[&str]) -> Workspace {
        let platforms_inline = platforms
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml = format!(
            "[workspace]\nname = \"install-platform-test\"\nchannels = []\nplatforms = [{platforms_inline}]\n",
        );
        Workspace::from_str(Path::new("pixi.toml"), &toml).unwrap()
    }

    /// No `--platform` flag -- the caller falls back to the env's
    /// host-aware platform selection.
    #[test]
    fn unset_platform_returns_none() {
        let workspace = workspace_with_platforms(&["linux-64"]);
        assert!(
            resolve_install_platform(&workspace, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn known_platform_resolves_to_its_workspace_name() {
        let workspace = workspace_with_platforms(&["linux-64"]);
        let name = "linux-64".parse().unwrap();
        let resolved = resolve_install_platform(&workspace, Some(&name))
            .unwrap()
            .unwrap();
        assert_eq!(resolved.as_str(), "linux-64");
    }

    /// A workspace platform whose subdir is not a candidate for the host
    /// still resolves -- the warning is emitted at run time but the
    /// resolution itself succeeds so install can proceed.
    #[test]
    fn cross_platform_subdir_resolves() {
        let workspace = workspace_with_platforms(&["linux-64", "osx-arm64"]);
        let target = if Platform::current() == Platform::OsxArm64 {
            "linux-64"
        } else {
            "osx-arm64"
        };
        let name = target.parse().unwrap();
        let resolved = resolve_install_platform(&workspace, Some(&name))
            .unwrap()
            .unwrap();
        assert_eq!(resolved.as_str(), target);
    }

    #[test]
    fn invalid_name_errors() {
        let workspace = workspace_with_platforms(&["linux-64"]);
        let name = "definitely-not-a-platform".parse().unwrap();
        let err = resolve_install_platform(&workspace, Some(&name)).unwrap_err();
        assert!(
            format!("{err}").contains("definitely-not-a-platform"),
            "expected the error to name the offending value, got: {err}",
        );
    }
}
