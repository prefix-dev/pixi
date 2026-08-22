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

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    str::FromStr,
};

use indexmap::IndexSet;
use rattler_conda_types::{GenericVirtualPackage, Platform};

use crate::{
    Feature, PixiPlatform, PixiPlatformName, TomlError, error::GenericError,
    toml::platform::synthesize_name_string,
};

/// Resolve a feature-referenced platform name: a workspace platform wins,
/// otherwise the name is treated as a bare conda subdir. `None` when it is
/// neither (parsing already validated the reference, so only a name that
/// vanished from the workspace ends up here).
pub fn resolve_referenced_platform<'a>(
    name: &PixiPlatformName,
    workspace_platforms: &'a IndexSet<PixiPlatform>,
) -> Option<Cow<'a, PixiPlatform>> {
    if let Some(platform) = workspace_platforms.iter().find(|p| p.name() == name) {
        return Some(Cow::Borrowed(platform));
    }
    Platform::from_str(name.as_str())
        .ok()
        .map(|subdir| Cow::Owned(PixiPlatform::from_subdir(subdir)))
}

/// Subdir-only variant of [`resolve_referenced_platform`] for callers that
/// don't need the platform itself (skips materialising a subdir platform).
pub(crate) fn resolve_referenced_subdir(
    name: &PixiPlatformName,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> Option<Platform> {
    if let Some(platform) = workspace_platforms.iter().find(|p| p.name() == name) {
        return Some(platform.subdir());
    }
    Platform::from_str(name.as_str()).ok()
}

/// The subdirs `feature` restricts its environments to, in reference order:
/// `None` when it spans every subdir (no `platforms` key, or a list the
/// `[system-requirements]` migration synthesised for a keyless feature).
fn referenced_subdirs(
    feature: &Feature,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> Option<IndexSet<Platform>> {
    let names = feature.referenced_platforms()?;
    Some(
        names
            .iter()
            .filter_map(|name| resolve_referenced_subdir(name, workspace_platforms))
            .collect(),
    )
}

/// Whether `feature` applies on `subdir`.
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

/// The distinct platforms the features pin for `subdir`, in first-seen
/// order. Features without a `platforms` key pin nothing; migration-
/// synthesised lists pin like user-written ones (they only differ in not
/// restricting subdirs), so this reads `platforms` raw.
fn referenced_platforms<'a>(
    features: &[&Feature],
    subdir: Platform,
    workspace_platforms: &'a IndexSet<PixiPlatform>,
) -> Vec<Cow<'a, PixiPlatform>> {
    let mut seen: HashSet<&PixiPlatformName> = HashSet::new();
    features
        .iter()
        .filter_map(|feature| feature.platforms.as_ref())
        .flatten()
        // A name always resolves to a platform of that name, so deduping
        // before resolving is equivalent and avoids cloning the names.
        .filter(|name| seen.insert(name))
        .filter_map(|name| resolve_referenced_platform(name, workspace_platforms))
        .filter(|platform| platform.subdir() == subdir)
        .collect()
}

/// Union the customised virtual packages of `platforms`, the declared entries
/// minus the subdir defaults, keyed by name with the highest version winning.
/// Ordered by name so the composed name is stable.
///
/// The materialised subdir defaults must not participate: a bare subdir
/// platform pinned by a feature carries them as declared entries, and the
/// default `__glibc=2.28` would override an explicit `libc = "2.17"` from
/// another feature. As in the legacy system-requirements union, a platform
/// that does not customise a virtual package does not constrain it.
fn union_virtual_packages(platforms: &[Cow<'_, PixiPlatform>]) -> Vec<GenericVirtualPackage> {
    let mut union: BTreeMap<String, GenericVirtualPackage> = BTreeMap::new();
    for package in platforms.iter().flat_map(|platform| {
        let subdir = platform.subdir();
        platform
            .declared_virtual_packages()
            .iter()
            .filter(move |gvp| !crate::platform::is_subdir_default(gvp, subdir))
    }) {
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
/// synthesised from the union when several are pinned. When the union is
/// empty the name collapses to the bare subdir, whose platform carries
/// exactly the same virtual packages.
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
            synthesize_name_string(subdir, &union)
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
        [single] => Ok(single.clone().into_owned()),
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

/// The subdirs an environment made of `features` resolves to: the declared
/// subdirs plus the subdirs the features themselves reference, narrowed to
/// what every feature supports. Feature-referenced subdirs stay scoped to
/// the environments using that feature (prefix-dev/pixi#6770).
pub(crate) fn environment_subdirs(
    features: &[&Feature],
    declared_subdirs: &IndexSet<Platform>,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> IndexSet<Platform> {
    // Resolve each feature's referenced subdirs once; the sets both widen
    // the environment and narrow it to what every feature supports.
    let referenced: Vec<Option<IndexSet<Platform>>> = features
        .iter()
        .map(|feature| referenced_subdirs(feature, workspace_platforms))
        .collect();
    let mut subdirs = declared_subdirs.clone();
    for set in referenced.iter().flatten() {
        subdirs.extend(set.iter().copied());
    }
    subdirs
        .into_iter()
        .filter(|subdir| {
            referenced
                .iter()
                .all(|set| set.as_ref().is_none_or(|set| set.contains(subdir)))
        })
        .collect()
}

/// Compose one [`PixiPlatform`] per subdir the environment resolves to.
pub(crate) fn combined_platforms(
    features: &[&Feature],
    declared_subdirs: &IndexSet<Platform>,
    workspace_platforms: &IndexSet<PixiPlatform>,
) -> Result<Vec<PixiPlatform>, TomlError> {
    environment_subdirs(features, declared_subdirs, workspace_platforms)
        .into_iter()
        .map(|subdir| combined_platform(features, subdir, workspace_platforms))
        .collect()
}
