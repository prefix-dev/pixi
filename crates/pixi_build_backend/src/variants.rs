use pixi_build_types as pbt;
use rattler_conda_types::VersionSpec;

pub use rattler_build_types::NormalizedKey;

/// Returns true if the specified [`pbt::PackageSpec`] is a valid variant
/// spec.
///
/// At the moment, a spec that allows any version is considered a variant spec.
pub fn can_be_used_as_variant(spec: &pbt::PackageSpec) -> bool {
    match spec {
        pbt::PackageSpec::Binary(spec) => {
            let pbt::BinaryPackageSpec {
                version,
                build,
                build_number,
                file_name,
                channel,
                subdir,
                md5,
                sha256,
                url,
                license,
            } = spec;

            version == &Some(VersionSpec::Any)
                && build.is_none()
                && build_number.is_none()
                && file_name.is_none()
                && channel.is_none()
                && subdir.is_none()
                && md5.is_none()
                && sha256.is_none()
                && url.is_none()
                && license.is_none()
        }
        _ => false,
    }
}
