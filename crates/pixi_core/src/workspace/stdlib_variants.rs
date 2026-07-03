//! Derive conda `c_stdlib` build variants from a platform's system
//! requirements (recorded as declared virtual packages).

use pixi_manifest::PixiPlatform;
use pixi_utils::variants::VariantValue;
use rattler_conda_types::ChannelUrl;

/// conda build-variant key naming the C stdlib provider for a build.
const C_STDLIB: &str = "c_stdlib";
/// conda build-variant key carrying the minimum C stdlib version.
const C_STDLIB_VERSION: &str = "c_stdlib_version";
/// conda-forge channel name. The providers below are conda-forge packages, so
/// the derivation only applies when the workspace builds against this channel.
const CONDA_FORGE: &str = "conda-forge";

/// Whether any resolved channel is conda-forge.
///
/// Matched on the final non-empty path segment of the resolved channel URL, so
/// both the bare name `conda-forge` and full forms like
/// `https://prefix.dev/conda-forge` count.
fn channels_target_conda_forge<'a>(channels: impl IntoIterator<Item = &'a ChannelUrl>) -> bool {
    channels.into_iter().any(|channel| {
        channel
            .url()
            .path_segments()
            .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
            == Some(CONDA_FORGE)
    })
}

/// Translate a platform's declared system requirements into the
/// `c_stdlib`/`c_stdlib_version` build-variant pair.
///
/// The variant *keys* are generic conda build-variant keys; only the providers
/// they map to (`macosx_deployment_target`, `sysroot`) are conda-forge
/// conventions. Build backends pin the minimum OS/libc target through the
/// `stdlib('c')` recipe function, which resolves against these two variants.
/// Pixi already records the target as virtual packages on the [`PixiPlatform`]
/// (`__osx` / `__glibc`), so we derive the variant pair from those instead of
/// making users spell it out by hand in `[workspace.build-variants]`:
///
/// | subdir    | virtual package | `c_stdlib`                 | `c_stdlib_version`    |
/// |-----------|-----------------|----------------------------|-----------------------|
/// | `osx-*`   | `__osx`         | `macosx_deployment_target` | the `__osx` version   |
/// | `linux-*` | `__glibc`       | `sysroot`                  | the `__glibc` version |
///
/// Because the providers are conda-forge packages, this returns an empty vec
/// unless one of `channels` is conda-forge. It also returns empty on Windows
/// (no meaningful stdlib version) and when the platform declares no matching
/// virtual package.
///
// TODO(#6175): handle `__musl` (musl libc) and `__cuda` -> `cuda_compiler_version`.
pub(crate) fn derive_stdlib_variants<'a>(
    platform: &PixiPlatform,
    channels: impl IntoIterator<Item = &'a ChannelUrl>,
) -> Vec<(String, VariantValue)> {
    if !channels_target_conda_forge(channels) {
        return Vec::new();
    }

    let subdir = platform.subdir();
    let (vp_name, provider) = if subdir.is_osx() {
        ("__osx", "macosx_deployment_target")
    } else if subdir.is_linux() {
        ("__glibc", "sysroot")
    } else {
        return Vec::new();
    };

    let Some(declared) = platform
        .declared_virtual_packages()
        .iter()
        .find(|vp| vp.name.as_normalized() == vp_name)
    else {
        return Vec::new();
    };

    vec![
        (
            C_STDLIB.to_string(),
            VariantValue::String(provider.to_string()),
        ),
        (
            C_STDLIB_VERSION.to_string(),
            VariantValue::String(declared.version.to_string()),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use pixi_manifest::{PixiPlatform, PixiPlatformName};
    use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
    use url::Url;

    use super::*;

    fn vp(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: name.parse().unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: "0".to_string(),
        }
    }

    /// Build a rich (named) platform declaring exactly `packages`.
    fn rich(name: &str, subdir: Platform, packages: Vec<GenericVirtualPackage>) -> PixiPlatform {
        PixiPlatform::new(PixiPlatformName::from_str(name).unwrap(), subdir, packages).unwrap()
    }

    fn channel(url: &str) -> Vec<ChannelUrl> {
        vec![ChannelUrl::from(Url::parse(url).unwrap())]
    }

    fn conda_forge() -> Vec<ChannelUrl> {
        channel("https://conda.anaconda.org/conda-forge")
    }

    fn into_map(
        pairs: Vec<(String, VariantValue)>,
    ) -> std::collections::BTreeMap<String, VariantValue> {
        pairs.into_iter().collect()
    }

    #[test]
    fn osx_derives_macosx_deployment_target() {
        let platform = rich("mac", Platform::OsxArm64, vec![vp("__osx", "13.5")]);
        let variants = into_map(derive_stdlib_variants(&platform, &conda_forge()));

        assert_eq!(
            variants.get("c_stdlib"),
            Some(&VariantValue::String(
                "macosx_deployment_target".to_string()
            ))
        );
        assert_eq!(
            variants.get("c_stdlib_version"),
            Some(&VariantValue::String("13.5".to_string()))
        );
    }

    #[test]
    fn linux_derives_sysroot_from_glibc() {
        let platform = rich("lin", Platform::Linux64, vec![vp("__glibc", "2.28")]);
        let variants = into_map(derive_stdlib_variants(&platform, &conda_forge()));

        assert_eq!(
            variants.get("c_stdlib"),
            Some(&VariantValue::String("sysroot".to_string()))
        );
        assert_eq!(
            variants.get("c_stdlib_version"),
            Some(&VariantValue::String("2.28".to_string()))
        );
    }

    #[test]
    fn windows_skips() {
        let platform = rich("windows-rich", Platform::Win64, vec![vp("__win", "10.0")]);
        assert!(derive_stdlib_variants(&platform, &conda_forge()).is_empty());
    }

    /// A linux platform that declares no `__glibc` (only an unrelated VP)
    /// produces nothing rather than inventing a version.
    #[test]
    fn linux_without_glibc_skips() {
        let platform = rich("lin-cuda", Platform::Linux64, vec![vp("__cuda", "12.0")]);
        assert!(derive_stdlib_variants(&platform, &conda_forge()).is_empty());
    }

    /// Bare subdir platforms carry pixi's portable defaults (`__glibc=2.28`,
    /// `__osx=13.0`), so derivation works off them too -- no rich platform
    /// required.
    #[test]
    fn bare_subdir_derives_from_defaults() {
        let linux = into_map(derive_stdlib_variants(
            &PixiPlatform::from_subdir(Platform::Linux64),
            &conda_forge(),
        ));
        assert_eq!(
            linux.get("c_stdlib_version"),
            Some(&VariantValue::String("2.28".to_string()))
        );

        let osx = into_map(derive_stdlib_variants(
            &PixiPlatform::from_subdir(Platform::OsxArm64),
            &conda_forge(),
        ));
        assert_eq!(
            osx.get("c_stdlib_version"),
            Some(&VariantValue::String("13.0".to_string()))
        );
    }

    /// conda-forge referenced by full URL still gates the derivation on.
    #[test]
    fn conda_forge_by_url_derives() {
        let platform = rich("mac", Platform::OsxArm64, vec![vp("__osx", "13.5")]);
        let variants = into_map(derive_stdlib_variants(
            &platform,
            &channel("https://prefix.dev/conda-forge"),
        ));
        assert_eq!(
            variants.get("c_stdlib"),
            Some(&VariantValue::String(
                "macosx_deployment_target".to_string()
            ))
        );
    }

    /// The providers are conda-forge packages, so a workspace that doesn't build
    /// against conda-forge derives nothing.
    #[test]
    fn non_conda_forge_channel_skips() {
        let platform = rich("mac", Platform::OsxArm64, vec![vp("__osx", "13.5")]);
        assert!(
            derive_stdlib_variants(&platform, &channel("https://prefix.dev/my-channel")).is_empty()
        );
    }

    #[test]
    fn no_channels_skips() {
        let platform = rich("mac", Platform::OsxArm64, vec![vp("__osx", "13.5")]);
        assert!(derive_stdlib_variants(&platform, &[]).is_empty());
    }
}
