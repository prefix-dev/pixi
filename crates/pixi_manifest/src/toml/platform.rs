use pixi_toml::TomlEnum;
use rattler_conda_types::Platform;
use toml_span::{DeserError, Deserialize, Value};

/// This type is used to represent the platform in the manifest file. The
/// [`Platform`] type from rattler contains more platforms than we actually
/// support like `noarch`. And this type allows us to alias some common
/// misspellings.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, strum::EnumString, strum::Display, strum::VariantNames,
)]
#[strum(serialize_all = "kebab-case")]
pub enum TomlPlatform {
    #[strum(serialize = "linux-32")]
    Linux32,
    #[strum(serialize = "linux-64")]
    Linux64,

    LinuxAarch64,
    LinuxArmv6l,
    LinuxArmv7l,
    LinuxPpc64le,
    LinuxPpc64,

    #[strum(serialize = "linux-s390x")]
    LinuxS390X,

    LinuxRiscv32,
    LinuxRiscv64,

    #[strum(serialize = "osx-64")]
    Osx64,
    OsxArm64,

    #[strum(serialize = "win-32")]
    Win32,
    #[strum(serialize = "win-64")]
    Win64,
    WinArm64,

    EmscriptenWasm32,
    WasiWasm32,

    ZosZ,
}

impl From<TomlPlatform> for Platform {
    fn from(value: TomlPlatform) -> Self {
        match value {
            TomlPlatform::Linux32 => Platform::Linux32,
            TomlPlatform::Linux64 => Platform::Linux64,
            TomlPlatform::LinuxAarch64 => Platform::LinuxAarch64,
            TomlPlatform::LinuxArmv6l => Platform::LinuxArmV6l,
            TomlPlatform::LinuxArmv7l => Platform::LinuxArmV7l,
            TomlPlatform::LinuxPpc64le => Platform::LinuxPpc64le,
            TomlPlatform::LinuxPpc64 => Platform::LinuxPpc64,
            TomlPlatform::LinuxS390X => Platform::LinuxS390X,
            TomlPlatform::LinuxRiscv32 => Platform::LinuxRiscv32,
            TomlPlatform::LinuxRiscv64 => Platform::LinuxRiscv64,
            TomlPlatform::Osx64 => Platform::Osx64,
            TomlPlatform::OsxArm64 => Platform::OsxArm64,
            TomlPlatform::Win32 => Platform::Win32,
            TomlPlatform::Win64 => Platform::Win64,
            TomlPlatform::WinArm64 => Platform::WinArm64,
            TomlPlatform::EmscriptenWasm32 => Platform::EmscriptenWasm32,
            TomlPlatform::WasiWasm32 => Platform::WasiWasm32,
            TomlPlatform::ZosZ => Platform::ZosZ,
        }
    }
}

impl<'de> Deserialize<'de> for TomlPlatform {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

impl<'de> pixi_toml::DeserializeAs<'de, Platform> for TomlPlatform {
    fn deserialize_as(value: &mut Value<'de>) -> Result<Platform, DeserError> {
        TomlPlatform::deserialize(value).map(Platform::from)
    }
}

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;
    use strum::VariantNames;

    use super::*;

    #[test]
    fn test_deserialize() {
        assert_debug_snapshot!(TomlPlatform::VARIANTS);
    }
}
