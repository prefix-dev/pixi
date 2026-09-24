//! Shared platform-name resolution for the dependency and task CLI paths.
//!
//! Pixi's CLI keeps subdirs and virtual packages out of the user-facing
//! vocabulary: a `--platform <NAME>` flag accepts a workspace-declared
//! [`PixiPlatform`] name, and falls back silently to parsing the value as a
//! conda subdir so users don't have to pre-declare a platform they just
//! want to scope a single dep or task to.
//!
//! Both `add`/`remove` and `task add`/`task remove`/`task alias` use this
//! helper so the resolution rules stay in lock-step. Callers that want to
//! auto-declare the resolved subdir-platform in the workspace (the way
//! `pixi add --platform linux-64` does) do so explicitly via
//! [`pixi_manifest::WorkspaceManifestMut::add_platforms`] after calling
//! [`resolve_platforms`]; non-mutating callers (`remove`, `task remove`)
//! just use the returned `Vec` and leave the manifest alone.
//!
//! Commands that read *solved* state instead of editing the manifest
//! (`list`, `tree`, `install`) must not use [`resolve_platforms`]: a bare
//! subdir has to name the platform the workspace actually declared for that
//! subdir, virtual packages included, or the lookup misses the lock-file row
//! and the installable environment. Those callers use
//! [`resolve_declared_platform`].

use indexmap::IndexSet;
use pixi_core::Workspace;
use pixi_manifest::{HasWorkspaceManifest, PixiPlatform, PixiPlatformName};
use rattler_conda_types::Platform;

/// Resolve each requested platform name against the workspace's declared
/// platforms. A name that is not a declared workspace platform but parses
/// as a bare conda subdir is accepted and returned as a fresh subdir
/// [`PixiPlatform`] (constructed via [`PixiPlatform::from_subdir`]).
///
/// The result is *not* added to the workspace -- that's the caller's
/// decision. Returns an error only when neither lookup nor subdir parsing
/// succeeds, so the same UX applies whether the caller is a manifest
/// mutator or a read-only lookup.
pub fn resolve_platforms(
    workspace_platforms: &IndexSet<PixiPlatform>,
    names: &[PixiPlatformName],
) -> miette::Result<Vec<PixiPlatform>> {
    names
        .iter()
        .map(|name| {
            if let Some(platform) = workspace_platforms.iter().find(|p| p.name() == name) {
                return Ok(platform.clone());
            }
            name.as_str()
                .parse::<Platform>()
                .map(PixiPlatform::from_subdir)
                .map_err(|_| miette::miette!("workspace does not define a platform named '{name}'"))
        })
        .collect()
}

/// Resolve a `--platform` value for a command that reads solved state
/// (`pixi list`, `pixi tree`, `pixi install`) rather than editing the
/// manifest.
///
/// A declared platform name matches first. A value that is not a declared
/// name but parses as a conda subdir resolves to the platform the workspace
/// declares *for that subdir* -- which is usually not
/// [`PixiPlatform::from_subdir`], because a `platforms` entry carrying
/// virtual packages gets a name synthesized from them
/// (`{ platform = "linux-64", glibc = "2.34" }` is named `linux-64-glibc-2-34`)
/// and it is that name, not the subdir, that keys the environment's platform
/// set and the `pixi.lock` platform table. Reaching for the subdir baseline
/// instead fabricates a platform the workspace never declared, so every
/// lookup keyed by it comes back empty.
///
/// Subdirs the workspace does not declare at all still fall back to the
/// subdir baseline, so cross-platform reads keep working.
pub fn resolve_declared_platform(
    workspace: &Workspace,
    name: &PixiPlatformName,
) -> miette::Result<PixiPlatform> {
    let workspace_platforms = &workspace.workspace_manifest().workspace.platforms;
    if let Some(platform) = workspace_platforms.iter().find(|p| p.name() == name) {
        return Ok(platform.clone());
    }
    let subdir = name
        .as_str()
        .parse::<Platform>()
        .map_err(|_| miette::miette!("workspace does not define a platform named '{name}'"))?;
    Ok(workspace.pixi_platform_for_subdir(subdir))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn workspace(platforms_inline: &str) -> Workspace {
        let toml = format!(
            "[workspace]\nname = \"platform-resolution-test\"\nchannels = []\nplatforms = [{platforms_inline}]\n",
        );
        Workspace::from_str(Path::new("pixi.toml"), &toml).unwrap()
    }

    fn resolve(workspace: &Workspace, name: &str) -> miette::Result<String> {
        let name: PixiPlatformName = name.parse().unwrap();
        resolve_declared_platform(workspace, &name).map(|p| p.name().as_str().to_string())
    }

    /// The reported case: a `platforms` entry with a `glibc` override is named
    /// after the override, so `--platform linux-riscv64` has to map onto that
    /// name -- resolving to the subdir baseline instead misses both the
    /// environment and the lock-file row.
    ///
    /// The override has to differ from the subdir's own `__glibc` default
    /// (2.39 on `linux-riscv64`), because a declaration that merely restates a
    /// default is not a customisation: the name collapses to the bare subdir
    /// and the entry is then rejected outright for carrying virtual packages.
    #[test]
    fn bare_subdir_resolves_to_the_declared_platform() {
        let workspace = workspace(r#""osx-arm64", { platform = "linux-riscv64", glibc = "2.41" }"#);
        assert_eq!(
            resolve(&workspace, "linux-riscv64").unwrap(),
            "linux-riscv64-glibc-2-41"
        );
    }

    /// An explicitly named entry is reachable both by its name and by its
    /// bare subdir.
    #[test]
    fn explicit_name_and_its_subdir_both_resolve() {
        let workspace =
            workspace(r#"{ name = "riscv", platform = "linux-riscv64", glibc = "2.41" }"#);
        assert_eq!(resolve(&workspace, "riscv").unwrap(), "riscv");
        assert_eq!(resolve(&workspace, "linux-riscv64").unwrap(), "riscv");
    }

    /// A declared subdir platform keeps resolving to itself, virtual-package
    /// defaults included.
    #[test]
    fn declared_subdir_platform_resolves_to_itself() {
        let workspace = workspace(r#""linux-64", "osx-arm64""#);
        assert_eq!(resolve(&workspace, "linux-64").unwrap(), "linux-64");
    }

    /// A subdir the workspace does not declare still resolves to the subdir
    /// baseline, so cross-platform reads are unaffected.
    #[test]
    fn undeclared_subdir_falls_back_to_the_baseline() {
        let workspace = workspace(r#""linux-64""#);
        assert_eq!(resolve(&workspace, "win-64").unwrap(), "win-64");
    }

    #[test]
    fn a_value_that_is_neither_a_name_nor_a_subdir_errors() {
        let workspace = workspace(r#""linux-64""#);
        let err = resolve(&workspace, "definitely-not-a-platform").unwrap_err();
        assert!(
            format!("{err}").contains("definitely-not-a-platform"),
            "expected the error to name the offending value, got: {err}",
        );
    }

    /// `resolve_platforms` -- the manifest-editing path -- deliberately keeps
    /// the bare subdir the user typed, so `pixi add`/`pixi task add` write a
    /// `[target.<subdir>]` selector rather than one keyed by a synthesized
    /// platform name.
    #[test]
    fn resolve_platforms_still_keeps_the_bare_subdir() {
        let workspace = workspace(r#"{ platform = "linux-riscv64", glibc = "2.41" }"#);
        let declared = (&workspace)
            .workspace_manifest()
            .workspace
            .platforms
            .clone();
        let name: PixiPlatformName = "linux-riscv64".parse().unwrap();
        let resolved = resolve_platforms(&declared, std::slice::from_ref(&name)).unwrap();
        assert_eq!(resolved[0].name().as_str(), "linux-riscv64");
    }
}
