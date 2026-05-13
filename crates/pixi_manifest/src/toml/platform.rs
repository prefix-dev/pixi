use std::str::FromStr;

use pixi_toml::TomlEnum;
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};
use serde::{Serialize, ser::SerializeMap};
use toml_span::{
    DeserError, Deserialize, Error, ErrorKind, Span, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{PixiPlatform, PixiPlatformName};

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

/// TOML representation of a workspace platform entry.
///
/// Supports two serializations:
///
/// ```toml
/// # Bare-string form (backwards-compatible): name == subdir, no virtual packages.
/// platforms = ["linux-64"]
///
/// # Inline-table form: a custom name with a subdir and virtual packages.
/// platforms = [
///   { name = "linux-64-cuda", subdir = "linux-64", virtual-packages = ["__cuda=12.0"] },
/// ]
/// ```
///
/// When the table form omits `subdir`, the `name` must itself parse as a conda
/// [`Platform`]. Each entry in `virtual-packages` is parsed as a
/// [`GenericVirtualPackage`] (`name[=version[=build_string]]`).
pub struct TomlPixiPlatform(pub PixiPlatform);

impl TomlPixiPlatform {
    pub fn into_inner(self) -> PixiPlatform {
        self.0
    }
}

impl<'de> Deserialize<'de> for TomlPixiPlatform {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(name) => {
                let subdir = Platform::from_str(&name).map_err(|e| Error {
                    kind: ErrorKind::Custom(e.to_string().into()),
                    span: value.span,
                    line_info: None,
                })?;
                Ok(TomlPixiPlatform(PixiPlatform::from_subdir(subdir)))
            }
            inner @ ValueInner::Table(_) => {
                let mut th = TableHelper::new(&mut Value::with_span(inner, value.span))?;
                let name_value: Spanned<String> = th.required("name")?;
                let subdir_str: Option<Spanned<String>> = th.optional("subdir");
                let virtual_packages_raw: Option<Vec<Spanned<String>>> =
                    th.optional("virtual-packages");
                th.finalize(None)?;

                let name = PixiPlatformName::try_from(name_value.value.as_str()).map_err(|_| {
                    Error {
                    kind: ErrorKind::Custom(
                        format!(
                            "'{}' is not a valid platform name (allowed: alphanumeric, '_', '-')",
                            name_value.value,
                        )
                        .into(),
                    ),
                    span: name_value.span,
                    line_info: None,
                }
                })?;

                let subdir = match subdir_str {
                    Some(s) => Platform::from_str(&s.value).map_err(|e| Error {
                        kind: ErrorKind::Custom(e.to_string().into()),
                        span: s.span,
                        line_info: None,
                    })?,
                    None => Platform::from_str(name.as_str()).map_err(|_| Error {
                        kind: ErrorKind::Custom(
                            format!(
                                "'{}' is not a conda subdir; specify 'subdir' explicitly when using a custom platform name", name.as_str()
                            )
                            .into(),
                        ),
                        span: name_value.span,
                        line_info: None,
                    })?,
                };

                let declared_virtual_packages = match virtual_packages_raw {
                    Some(specs) => parse_virtual_packages(specs)?,
                    None => Vec::new(),
                };

                Ok(TomlPixiPlatform(PixiPlatform::new(
                    name,
                    subdir,
                    declared_virtual_packages,
                )))
            }
            other => Err(expected("a string or table", other, value.span).into()),
        }
    }
}

impl<'de> pixi_toml::DeserializeAs<'de, PixiPlatform> for TomlPixiPlatform {
    fn deserialize_as(value: &mut Value<'de>) -> Result<PixiPlatform, DeserError> {
        TomlPixiPlatform::deserialize(value).map(TomlPixiPlatform::into_inner)
    }
}

impl Serialize for TomlPixiPlatform {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let platform = &self.0;
        let name = platform.name().as_str();
        let subdir_str = platform.subdir().to_string();
        let virtual_packages: Vec<String> = platform
            .declared_virtual_packages()
            .iter()
            .map(format_virtual_package)
            .collect();
        let virtual_packages_are_default = virtual_packages.is_empty();

        if name == subdir_str && virtual_packages_are_default {
            return serializer.serialize_str(name);
        }

        let needs_subdir = name != subdir_str;
        let entries = 1 + usize::from(needs_subdir) + usize::from(!virtual_packages_are_default);
        let mut map = serializer.serialize_map(Some(entries))?;
        map.serialize_entry("name", name)?;
        if needs_subdir {
            map.serialize_entry("subdir", &subdir_str)?;
        }
        if !virtual_packages_are_default {
            map.serialize_entry("virtual-packages", &virtual_packages)?;
        }
        map.end()
    }
}

fn parse_virtual_packages(
    specs: Vec<Spanned<String>>,
) -> Result<Vec<GenericVirtualPackage>, DeserError> {
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let gvp = parse_generic_virtual_package(&spec.value, spec.span)?;
        if !gvp.name.as_normalized().starts_with("__") {
            return Err(Error {
                kind: ErrorKind::Custom(
                    format!(
                        "'{}' is not a recognized virtual package (must start with '__')",
                        gvp.name.as_normalized()
                    )
                    .into(),
                ),
                span: spec.span,
                line_info: None,
            }
            .into());
        }
        out.push(gvp);
    }
    Ok(out)
}

fn parse_generic_virtual_package(s: &str, span: Span) -> Result<GenericVirtualPackage, Error> {
    let mut parts = s.split('=');
    let name_str = parts.next().unwrap_or("");
    let name = PackageName::try_from(name_str).map_err(|e| Error {
        kind: ErrorKind::Custom(
            format!("'{name_str}' is not a valid virtual package name: {e}").into(),
        ),
        span,
        line_info: None,
    })?;
    let version_str = parts.next().unwrap_or("0");
    let version = Version::from_str(version_str).map_err(|e| Error {
        kind: ErrorKind::Custom(
            format!("'{version_str}' is not a valid virtual package version: {e}").into(),
        ),
        span,
        line_info: None,
    })?;
    let build_string = parts.next().unwrap_or("").to_string();
    Ok(GenericVirtualPackage {
        name,
        version,
        build_string,
    })
}

/// Render a `GenericVirtualPackage` as the shortest conda spec that
/// round-trips through [`parse_generic_virtual_package`]: drop a zero
/// `build_string` and, when also zero, the version.
fn format_virtual_package(gvp: &GenericVirtualPackage) -> String {
    let name = gvp.name.as_normalized();
    let version_is_zero = gvp.version == Version::major(0);
    let build_is_zero = gvp.build_string.is_empty() || gvp.build_string == "0";

    if version_is_zero && build_is_zero {
        name.to_string()
    } else if build_is_zero {
        format!("{}={}", name, gvp.version)
    } else {
        gvp.to_string()
    }
}

#[cfg(test)]
mod test {
    use insta::{assert_debug_snapshot, assert_snapshot};
    use pixi_test_utils::format_parse_error;
    use strum::VariantNames;

    use super::*;
    use crate::toml::FromTomlStr;

    #[test]
    fn test_deserialize() {
        assert_debug_snapshot!(TomlPlatform::VARIANTS);
    }

    #[derive(Debug)]
    #[allow(dead_code)]
    struct TopLevel {
        platform: PixiPlatform,
    }

    impl<'de> Deserialize<'de> for TopLevel {
        fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
            let mut th = TableHelper::new(value)?;
            let platform = th.required::<TomlPixiPlatform>("platform")?.into_inner();
            th.finalize(None)?;
            Ok(TopLevel { platform })
        }
    }

    fn vp_strings(p: &PixiPlatform) -> Vec<String> {
        p.declared_virtual_packages()
            .iter()
            .map(format_virtual_package)
            .collect()
    }

    #[test]
    fn test_workspace_platform_bare_string() {
        let parsed = TopLevel::from_toml_str(r#"platform = "linux-64""#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "linux-64");
        assert_eq!(parsed.platform.subdir(), Platform::Linux64);
        assert!(vp_strings(&parsed.platform).is_empty());
    }

    #[test]
    fn test_workspace_platform_table_with_subdir() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { name = "linux-64-cuda", subdir = "linux-64", virtual-packages = ["__cuda=12.0"] }"#,
        )
        .unwrap();
        assert_eq!(parsed.platform.name().as_str(), "linux-64-cuda");
        assert_eq!(parsed.platform.subdir(), Platform::Linux64);
        assert_eq!(
            vp_strings(&parsed.platform),
            vec!["__cuda=12.0".to_string()]
        );
    }

    #[test]
    fn test_workspace_platform_table_inferred_subdir() {
        let parsed = TopLevel::from_toml_str(r#"platform = { name = "osx-arm64" }"#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "osx-arm64");
        assert_eq!(parsed.platform.subdir(), Platform::OsxArm64);
        assert!(vp_strings(&parsed.platform).is_empty());
    }

    #[test]
    fn test_workspace_platform_multiple_virtual_packages() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { name = "linux-64-cuda", subdir = "linux-64", virtual-packages = ["__cuda=12.0", "__glibc=2.28"] }"#,
        )
        .unwrap();
        assert_eq!(
            vp_strings(&parsed.platform),
            vec!["__cuda=12.0".to_string(), "__glibc=2.28".to_string()]
        );
    }

    /// Unknown `__<name>` entries (those rattler has no override slot for) are
    /// kept verbatim so the TOML layer doesn't need updating every time rattler
    /// learns about a new virtual package.
    #[test]
    fn test_workspace_platform_unknown_name_roundtrips() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { name = "linux-64", virtual-packages = ["__unix", "__future_pkg=1.2"] }"#,
        )
        .unwrap();
        assert_eq!(
            vp_strings(&parsed.platform),
            vec!["__unix".to_string(), "__future_pkg=1.2".to_string()]
        );
    }

    #[test]
    fn test_workspace_platform_invalid_name() {
        let input = r#"platform = { name = "linux 64" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, error), @r#"
         × 'linux 64' is not a valid platform name (allowed: alphanumeric, '_', '-')
          ╭─[pixi.toml:1:22]
        1 │ platform = { name = "linux 64" }
          ·                      ────────
          ╰────
        "#);
    }

    #[test]
    fn test_workspace_platform_custom_name_without_subdir() {
        let input = r#"platform = { name = "linux-64-cuda" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, error), @r#"
         × 'linux-64-cuda' is not a conda subdir; specify 'subdir' explicitly when using a custom platform name
          ╭─[pixi.toml:1:22]
        1 │ platform = { name = "linux-64-cuda" }
          ·                      ─────────────
          ╰────
        "#);
    }

    #[test]
    fn test_workspace_platform_unknown_subdir() {
        let input = r#"platform = "bogus-platform""#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        // Use a contains check rather than a full snapshot because the inner
        // rattler_conda_types error wording can drift across versions.
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("bogus-platform"),
            "expected error to mention the bad subdir, got: {rendered}"
        );
    }

    #[test]
    fn test_workspace_platform_unknown_virtual_package() {
        let input = r#"platform = { name = "linux-64", virtual-packages = ["bogus"] }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("'bogus' is not a recognized virtual package"),
            "expected error to mention the bad virtual package, got: {rendered}"
        );
    }

    fn platform_with_packages(
        name: &str,
        subdir: Platform,
        declared: Vec<GenericVirtualPackage>,
    ) -> PixiPlatform {
        PixiPlatform::new(
            PixiPlatformName::try_from(name).expect("valid platform name"),
            subdir,
            declared,
        )
    }

    fn gvp(spec: &str) -> GenericVirtualPackage {
        parse_generic_virtual_package(spec, Span::new(0, 0)).expect("valid virtual package")
    }

    #[test]
    fn test_serialize_bare_string() {
        let platform = platform_with_packages("linux-64", Platform::Linux64, Vec::new());
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(json, serde_json::Value::String("linux-64".into()));
    }

    #[test]
    fn test_serialize_custom_name_with_subdir() {
        let platform =
            platform_with_packages("linux-64-cuda", Platform::Linux64, vec![gvp("__cuda=12.0")]);
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "linux-64-cuda",
                "subdir": "linux-64",
                "virtual-packages": ["__cuda=12.0"],
            })
        );
    }

    #[test]
    fn test_serialize_default_name_with_virtual_packages() {
        let platform =
            platform_with_packages("linux-64", Platform::Linux64, vec![gvp("__cuda=12.0")]);
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "linux-64",
                "virtual-packages": ["__cuda=12.0"],
            })
        );
    }

    #[test]
    fn test_serialize_custom_name_default_virtual_packages() {
        let platform = platform_with_packages("linux-64-cuda", Platform::Linux64, Vec::new());
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "linux-64-cuda",
                "subdir": "linux-64",
            })
        );
    }

    /// Round-trip: deserialize a TOML representation, re-serialize via
    /// `TomlPixiPlatform`, and check we get the same JSON shape back.
    #[test]
    fn test_roundtrip_table_form() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { name = "linux-64-cuda", subdir = "linux-64", virtual-packages = ["__cuda=12.0"] }"#,
        )
        .unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "linux-64-cuda",
                "subdir": "linux-64",
                "virtual-packages": ["__cuda=12.0"],
            })
        );
    }

    #[test]
    fn test_roundtrip_bare_string() {
        let parsed = TopLevel::from_toml_str(r#"platform = "linux-64""#).unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(json, serde_json::Value::String("linux-64".into()));
    }
}
