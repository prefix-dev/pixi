//! Per-environment platform composition.
//!
//! On the subdir-only `[system-requirements]` path each feature references the
//! rich platforms its system requirements synthesised (e.g. `linux-64-cuda-13-0`),
//! all registered on the workspace. An environment that combines such features
//! must solve for a single platform per subdir: when its features pin one rich
//! platform for a subdir that platform is reused, and when they pin several the
//! environment combines them into one carrying the union of their virtual
//! packages. Shared by the parse-time registration pass and
//! [`crate::FeaturesExt::platforms`].

use std::collections::{BTreeMap, HashSet};

use indexmap::IndexSet;
use rattler_conda_types::{GenericVirtualPackage, Platform};

use crate::{
    Feature, PixiPlatform, PixiPlatformName, TomlError, error::GenericError,
    toml::platform::synthesize_name_string,
};

/// The subdirs `feature` covers: `None` when it has no `platforms` key (every
/// subdir), otherwise the subdirs of the workspace platforms it references.
fn referenced_subdirs(
    feature: &Feature,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> Option<HashSet<Platform>> {
    let names = feature.platforms.as_ref()?;
    Some(
        names
            .iter()
            .filter_map(|name| workspace_platforms.iter().find(|p| p.name() == name))
            .map(PixiPlatform::subdir)
            .collect(),
    )
}

/// Whether `feature` applies on `subdir` (no `platforms` key means everywhere).
pub(crate) fn feature_supports_subdir(
    feature: &Feature,
    subdir: Platform,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> bool {
    match referenced_subdirs(feature, workspace_platforms) {
        None => true,
        Some(subdirs) => subdirs.contains(&subdir),
    }
}

/// Whether `feature` applies on `platform`'s subdir. Mirrors
/// [`Feature::supports_platform`] but matches by subdir so a feature pinned to
/// `linux-64-cuda-13-0` still applies to a composed `linux-64-cuda-13-0-glibc-…`.
pub(crate) fn feature_supports_platform(
    feature: &Feature,
    platform: &PixiPlatform,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> bool {
    feature_supports_subdir(feature, platform.subdir(), workspace_platforms)
}

/// The distinct workspace platforms the features pin for `subdir`, in first-seen
/// order. Features without a `platforms` key pin nothing.
fn referenced_platforms<'a>(
    features: &[&Feature],
    subdir: Platform,
    workspace_platforms: &'a IndexSet<PixiPlatform>,
) -> Vec<&'a PixiPlatform> {
    let mut seen: HashSet<&PixiPlatformName> = HashSet::new();
    features
        .iter()
        .filter_map(|feature| feature.platforms.as_ref())
        .flatten()
        .filter_map(|name| workspace_platforms.iter().find(|p| p.name() == name))
        .filter(|platform| platform.subdir() == subdir)
        .filter(|platform| seen.insert(platform.name()))
        .collect()
}

/// Union the declared virtual packages of `platforms`, keyed by name with the
/// highest version winning. Ordered by name so the composed name is stable.
fn union_virtual_packages(platforms: &[&PixiPlatform]) -> Vec<GenericVirtualPackage> {
    let mut union: BTreeMap<String, GenericVirtualPackage> = BTreeMap::new();
    for package in platforms
        .iter()
        .flat_map(|platform| platform.declared_virtual_packages())
    {
        union
            .entry(package.name.as_normalized().to_string())
            .and_modify(|existing| {
                if package.version > existing.version {
                    *existing = package.clone();
                }
            })
            .or_insert_with(|| package.clone());
    }
    union.into_values().collect()
}

/// The name of the platform `features` resolve to on `subdir`: the bare subdir
/// when nothing is pinned, the single pinned platform's name, or the name
/// synthesised from the union when several are pinned.
pub(crate) fn combined_platform_name(
    features: &[&Feature],
    subdir: Platform,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> String {
    let referenced = referenced_platforms(features, subdir, workspace_platforms);
    match referenced.as_slice() {
        [] => subdir.as_str().to_string(),
        [single] => single.name().as_str().to_string(),
        many => {
            let union = union_virtual_packages(many);
            let name = synthesize_name_string(subdir, &union);
            // A union that collapses to the subdir defaults can't reuse the
            // reserved bare-subdir name on a virtual-package-bearing platform.
            if !union.is_empty() && name == subdir.as_str() {
                format!("{name}-generic")
            } else {
                name
            }
        }
    }
}

/// The platform `features` resolve to on `subdir` (see
/// [`combined_platform_name`]); `None` when the union name is invalid.
fn combined_platform(
    features: &[&Feature],
    subdir: Platform,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> Result<PixiPlatform, TomlError> {
    let referenced = referenced_platforms(features, subdir, workspace_platforms);
    match referenced.as_slice() {
        [] => Ok(PixiPlatform::from_subdir(subdir)),
        [single] => Ok((*single).clone()),
        many => {
            let union = union_virtual_packages(many);
            let name = combined_platform_name(features, subdir, workspace_platforms);
            let name = PixiPlatformName::try_from(name.as_str()).map_err(|error| {
                TomlError::from(GenericError::new(format!(
                    "composed platform name '{name}' is not a valid pixi platform name: {error}"
                )))
            })?;
            PixiPlatform::new_with_defaults(name.clone(), subdir, union).map_err(|error| {
                TomlError::from(GenericError::new(format!(
                    "composed platform '{name}' is invalid: {error}"
                )))
            })
        }
    }
}

/// Compose one [`PixiPlatform`] per subdir every feature supports.
pub(crate) fn combined_platforms(
    features: &[&Feature],
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> Result<Vec<PixiPlatform>, TomlError> {
    let subdirs: IndexSet<Platform> = workspace_platforms
        .iter()
        .map(PixiPlatform::subdir)
        .collect();
    subdirs
        .into_iter()
        .filter(|subdir| {
            features
                .iter()
                .all(|feature| feature_supports_subdir(feature, *subdir, workspace_platforms))
        })
        .map(|subdir| combined_platform(features, subdir, workspace_platforms))
        .collect()
}
