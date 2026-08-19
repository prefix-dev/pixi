//! Shared platform-name resolution for the dependency and task CLI paths.
//!
//! Pixi's CLI keeps subdirs and virtual packages out of the user-facing
//! vocabulary: a `--platform <NAME>` flag accepts a workspace-declared
//! [`PixiPlatform`] name, and falls back silently to parsing the value as a
//! conda subdir so users don't have to pre-declare a platform they just
//! want to scope a single dep or task to.
//!
//! Both `add`/`remove` and `task add`/`task remove`/`task alias` use these
//! helpers so the resolution rules stay in lock-step. Callers that want to
//! auto-declare the resolved platforms in the workspace (the way
//! `pixi add --platform linux-64` does) do so via [`declare_platforms`];
//! non-mutating callers (`remove`, `task remove`) just use the
//! [`resolve_platforms`] result and leave the manifest alone.

use std::borrow::Cow;

use indexmap::IndexSet;
use pixi_core::workspace::WorkspaceMut;
use pixi_manifest::{
    FeatureName, HasWorkspaceManifest, PixiPlatform, PixiPlatformName, resolve_referenced_platform,
};

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
            resolve_referenced_platform(name, workspace_platforms)
                .map(Cow::into_owned)
                .ok_or_else(|| {
                    miette::miette!("workspace does not define a platform named '{name}'")
                })
        })
        .collect()
}

/// Declare `platforms` on `target`, skipping any that `feature` already
/// references by name: those are covered in that feature's environments, and
/// declaring them would widen every other environment (prefix-dev/pixi#6770).
pub fn declare_platforms(
    workspace: &mut WorkspaceMut,
    feature: &FeatureName,
    platforms: &[PixiPlatform],
    target: &FeatureName,
) -> miette::Result<()> {
    let referenced = workspace
        .workspace()
        .workspace_manifest()
        .feature(feature)
        .and_then(|f| f.referenced_platforms());
    let to_declare: Vec<&PixiPlatform> = platforms
        .iter()
        .filter(|p| !referenced.is_some_and(|names| names.contains(p.name())))
        .collect();
    workspace.manifest().add_platforms(to_declare, target)?;
    Ok(())
}
