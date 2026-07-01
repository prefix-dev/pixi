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

use indexmap::IndexSet;
use pixi_manifest::{PixiPlatform, PixiPlatformName};
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
