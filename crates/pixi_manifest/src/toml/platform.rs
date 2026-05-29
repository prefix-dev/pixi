use std::{collections::HashSet, str::FromStr};

use pixi_toml::TomlEnum;
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};
use serde::{Serialize, ser::SerializeMap};
use toml_span::{
    DeserError, Deserialize, Error, ErrorKind, Span, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{PixiPlatform, PixiPlatformName, platform::is_subdir_default};

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

/// How a friendly virtual-package key's value is mapped onto a
/// [`GenericVirtualPackage`].
#[derive(Debug, Clone, Copy)]
enum VirtualPackageValueKind {
    /// The value is a version string and lands in `GenericVirtualPackage::version`;
    /// `build_string` is left empty.
    Version,
    /// The value is a microarchitecture string and lands in `build_string`;
    /// `version` is forced to `0`. This is the shape upstream rattler expects
    /// for `__archspec`.
    Microarch,
}

/// A friendly TOML/CLI shortcut for a virtual package.
struct FriendlyVirtualPackage {
    /// Canonical key. This is the form pixi writes back when serializing.
    key: &'static str,
    /// Alternative input keys accepted as synonyms for [`Self::key`].
    aliases: &'static [&'static str],
    /// The conda virtual-package name the key maps to (e.g. `__osx`).
    conda_name: &'static str,
    kind: VirtualPackageValueKind,
}

/// Friendly TOML keys accepted inside an inline platform entry. Order is
/// load-bearing: the auto-derived platform name concatenates these keys in
/// this exact sequence so two manifests that declare the same packages in
/// different key order share a name.
const FRIENDLY_VIRTUAL_PACKAGES: &[FriendlyVirtualPackage] = &[
    FriendlyVirtualPackage {
        key: "cuda",
        aliases: &[],
        conda_name: "__cuda",
        kind: VirtualPackageValueKind::Version,
    },
    FriendlyVirtualPackage {
        key: "archspec",
        aliases: &[],
        conda_name: "__archspec",
        kind: VirtualPackageValueKind::Microarch,
    },
    FriendlyVirtualPackage {
        key: "libc",
        aliases: &[],
        conda_name: "__glibc",
        kind: VirtualPackageValueKind::Version,
    },
    FriendlyVirtualPackage {
        key: "linux",
        aliases: &[],
        conda_name: "__linux",
        kind: VirtualPackageValueKind::Version,
    },
    FriendlyVirtualPackage {
        key: "macos",
        aliases: &["osx"],
        conda_name: "__osx",
        kind: VirtualPackageValueKind::Version,
    },
    FriendlyVirtualPackage {
        key: "windows",
        aliases: &[],
        conda_name: "__win",
        kind: VirtualPackageValueKind::Version,
    },
];

/// TOML representation of a workspace platform entry.
///
/// Supports two serializations:
///
/// ```toml
/// # Bare-string form (backwards-compatible): name == subdir, no virtual packages.
/// platforms = ["linux-64"]
///
/// # Inline-table form: a conda subdir plus declared virtual packages.
/// platforms = [
///   { platform = "linux-64", cuda = "12.0", libc = "2.28" },
///   { name = "jetson-nano", platform = "linux-aarch64", cuda = "12.8", archspec = "armv8-a" },
/// ]
/// ```
///
/// In the inline-table form:
///
/// * `platform` carries the conda subdir. It can be omitted when `name` is
///   itself a valid conda subdir.
/// * `name` is the workspace-scoped label features and the lockfile reference
///   the entry by. It's optional; when omitted, it's auto-derived from
///   `platform` and the declared virtual packages so the entry still has a
///   stable identifier.
/// * Each remaining key is a virtual-package shortcut: `cuda`, `archspec`,
///   `libc`, `linux`, `macos` (alias `osx`), `windows`. Their values are conda
///   version strings (or, for `archspec`, a microarchitecture string). Any key
///   starting with `__` is taken as a raw `GenericVirtualPackage` so rattler
///   can grow new virtual packages without the TOML layer needing to learn
///   about them.
pub struct TomlPixiPlatform(pub PixiPlatform);

impl TomlPixiPlatform {
    pub fn into_inner(self) -> PixiPlatform {
        self.0
    }
}

impl<'de> Deserialize<'de> for TomlPixiPlatform {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => {
                let subdir = Platform::from_str(&s).map_err(|e| Error {
                    kind: ErrorKind::Custom(e.to_string().into()),
                    span: value.span,
                    line_info: None,
                })?;
                Ok(TomlPixiPlatform(PixiPlatform::from_subdir(subdir)))
            }
            inner @ ValueInner::Table(_) => {
                let table_span = value.span;
                let mut th = TableHelper::new(&mut Value::with_span(inner, table_span))?;

                let name_value: Option<Spanned<String>> = th.optional("name");
                let platform_value: Option<Spanned<String>> = th.optional("platform");

                let mut declared: Vec<GenericVirtualPackage> = Vec::new();
                for entry in FRIENDLY_VIRTUAL_PACKAGES {
                    if let Some(raw) = take_friendly_value(&mut th, entry)? {
                        declared.push(build_friendly_virtual_package(
                            entry.conda_name,
                            entry.kind,
                            &raw,
                        )?);
                    }
                }

                // Anything still in the table that starts with `__` is treated
                // as a raw virtual-package declaration for forward compat with
                // virtual packages we don't have a friendly key for yet.
                let raw_keys: Vec<String> = th
                    .table
                    .keys()
                    .filter(|k| k.name.starts_with("__"))
                    .map(|k| k.name.as_ref().to_owned())
                    .collect();
                for key_name in raw_keys {
                    let (key, mut entry_value) = th
                        .table
                        .remove_entry(key_name.as_str())
                        .expect("just enumerated");
                    declared.push(parse_raw_virtual_package(
                        key.name.as_ref(),
                        key.span,
                        &mut entry_value,
                    )?);
                }

                th.finalize(None)?;

                let subdir =
                    resolve_subdir(platform_value.as_ref(), name_value.as_ref(), table_span)?;

                let name = match name_value {
                    Some(n) => parse_pixi_platform_name(&n)?,
                    None => synthesize_name(subdir, &declared, table_span)?,
                };

                let platform =
                    PixiPlatform::new_with_defaults(name, subdir, declared).map_err(|e| {
                        Error {
                            kind: ErrorKind::Custom(
                                format!(
                                    "platform entry rejected: {e}; either drop the virtual packages or give the entry a `name` distinct from its subdir",
                                )
                                .into(),
                            ),
                            span: table_span,
                            line_info: None,
                        }
                    })?;
                Ok(TomlPixiPlatform(platform))
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
        let declared = platform.declared_virtual_packages();

        let (friendly, raw) = classify_virtual_packages(platform.subdir(), declared);
        // Subdir platforms (`name == subdir`) carry the subdir defaults, but
        // the defaults are filtered out by `classify_virtual_packages` so
        // `friendly` and `raw` end up empty -- the bare-string shape covers
        // them exactly. Falls through to the inline-table shape as soon as
        // there is a non-default virtual package or a custom name.
        if name == subdir_str && friendly.is_empty() && raw.is_empty() {
            return serializer.serialize_str(name);
        }

        let auto_name = synthesize_name_string(platform.subdir(), declared);
        let emit_name = name != auto_name;

        let entries = 1 + usize::from(emit_name) + friendly.len() + raw.len();
        let mut map = serializer.serialize_map(Some(entries))?;
        if emit_name {
            map.serialize_entry("name", name)?;
        }
        map.serialize_entry("platform", &subdir_str)?;
        for (key, value) in &friendly {
            map.serialize_entry(key, value)?;
        }
        for (key, value) in &raw {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// Take a friendly virtual-package value from the table, accepting either the
/// canonical key or any of its aliases. Errors if more than one spelling of
/// the same package is present.
fn take_friendly_value<'de>(
    th: &mut TableHelper<'de>,
    entry: &FriendlyVirtualPackage,
) -> Result<Option<Spanned<String>>, Error> {
    let mut found: Option<Spanned<String>> = None;
    for key in std::iter::once(entry.key).chain(entry.aliases.iter().copied()) {
        let Some(raw) = th.optional::<Spanned<String>>(key) else {
            continue;
        };
        if found.is_some() {
            return Err(Error {
                kind: ErrorKind::Custom(
                    format!("'{key}' is an alias for '{}'; set only one", entry.key).into(),
                ),
                span: raw.span,
                line_info: None,
            });
        }
        found = Some(raw);
    }
    Ok(found)
}

fn build_friendly_virtual_package(
    conda_name: &str,
    kind: VirtualPackageValueKind,
    raw: &Spanned<String>,
) -> Result<GenericVirtualPackage, Error> {
    let package_name =
        PackageName::try_from(conda_name).expect("static virtual-package name is valid");
    match kind {
        VirtualPackageValueKind::Version => {
            let version = Version::from_str(raw.value.as_str()).map_err(|e| Error {
                kind: ErrorKind::Custom(
                    format!("'{}' is not a valid version: {e}", raw.value).into(),
                ),
                span: raw.span,
                line_info: None,
            })?;
            Ok(GenericVirtualPackage {
                name: package_name,
                version,
                build_string: String::new(),
            })
        }
        VirtualPackageValueKind::Microarch => {
            if raw.value.is_empty() {
                return Err(Error {
                    kind: ErrorKind::Custom(
                        "'archspec' requires a non-empty microarchitecture string".into(),
                    ),
                    span: raw.span,
                    line_info: None,
                });
            }
            Ok(GenericVirtualPackage {
                name: package_name,
                version: Version::major(0),
                build_string: raw.value.clone(),
            })
        }
    }
}

/// Parse a `__name = "version[=build_string]"` entry as a
/// [`GenericVirtualPackage`]. Used for keys that don't have a friendly
/// shortcut so the TOML layer stays forward-compatible.
fn parse_raw_virtual_package(
    key: &str,
    key_span: Span,
    value: &mut Value<'_>,
) -> Result<GenericVirtualPackage, Error> {
    let name = PackageName::try_from(key).map_err(|e| Error {
        kind: ErrorKind::Custom(format!("'{key}' is not a valid virtual-package name: {e}").into()),
        span: key_span,
        line_info: None,
    })?;
    let value_span = value.span;
    let s = match value.take() {
        ValueInner::String(s) => s.into_owned(),
        other => {
            return Err(Error {
                kind: ErrorKind::Wanted {
                    expected: "a string",
                    found: other.type_str(),
                },
                span: value_span,
                line_info: None,
            });
        }
    };
    let mut parts = s.splitn(2, '=');
    let version_str = parts.next().unwrap_or("");
    let version = Version::from_str(version_str).map_err(|e| Error {
        kind: ErrorKind::Custom(
            format!("'{version_str}' is not a valid virtual-package version: {e}").into(),
        ),
        span: value_span,
        line_info: None,
    })?;
    let build_string = parts.next().unwrap_or("").to_string();
    Ok(GenericVirtualPackage {
        name,
        version,
        build_string,
    })
}

fn resolve_subdir(
    platform_value: Option<&Spanned<String>>,
    name_value: Option<&Spanned<String>>,
    table_span: Span,
) -> Result<Platform, DeserError> {
    if let Some(p) = platform_value {
        return Platform::from_str(&p.value).map_err(|e| {
            Error {
                kind: ErrorKind::Custom(e.to_string().into()),
                span: p.span,
                line_info: None,
            }
            .into()
        });
    }
    if let Some(n) = name_value {
        return Platform::from_str(&n.value).map_err(|_| {
            Error {
                kind: ErrorKind::Custom(
                    format!(
                        "'{}' is not a conda subdir; set 'platform' explicitly when using a custom name",
                        n.value,
                    )
                    .into(),
                ),
                span: n.span,
                line_info: None,
            }
            .into()
        });
    }
    Err(Error {
        kind: ErrorKind::Custom(
            "a platform entry must set at least one of 'name' or 'platform'".into(),
        ),
        span: table_span,
        line_info: None,
    }
    .into())
}

fn parse_pixi_platform_name(name: &Spanned<String>) -> Result<PixiPlatformName, DeserError> {
    PixiPlatformName::try_from(name.value.as_str()).map_err(|_| {
        Error {
            kind: ErrorKind::Custom(
                format!(
                    "'{}' is not a valid platform name (allowed: alphanumeric, '-')",
                    name.value,
                )
                .into(),
            ),
            span: name.span,
            line_info: None,
        }
        .into()
    })
}

fn synthesize_name(
    subdir: Platform,
    declared: &[GenericVirtualPackage],
    span: Span,
) -> Result<PixiPlatformName, DeserError> {
    let raw = synthesize_name_string(subdir, declared);
    PixiPlatformName::try_from(raw.as_str()).map_err(|e| {
        Error {
            kind: ErrorKind::Custom(
                format!(
                    "auto-derived platform name '{raw}' is not valid ({e}); set 'name' explicitly",
                )
                .into(),
            ),
            span,
            line_info: None,
        }
        .into()
    })
}

/// One entry in [`inline_virtual_package_specs`]'s return value.
///
/// Pairs the rendered `key=value` text (using friendly shortcuts where
/// possible, raw `__name=value` otherwise) with the underlying
/// [`GenericVirtualPackage`] the entry came from. The CLI uses the latter
/// to do identity/satisfaction checks against host-detected VPs without
/// having to re-parse the rendered form.
#[derive(Debug, Clone)]
pub struct InlineVirtualPackage {
    /// The conda virtual package the entry represents.
    pub package: GenericVirtualPackage,
    /// `key=value` rendering. Friendly keys (`cuda`, `archspec`, `libc`,
    /// `linux`, `macos`, `windows`) are used when the entry fits one;
    /// otherwise the raw `__name=value` form is used.
    pub rendered: String,
}

/// Render a platform's declared virtual packages as the inline `key=value`
/// strings used in `pixi.toml` and `pixi workspace platform add`, paired
/// with the underlying conda VP so callers can run match logic against
/// them.
///
/// Friendly entries use the `FRIENDLY_VIRTUAL_PACKAGES` short keys
/// (`cuda`, `archspec`, `libc`, ...), in canonical order. Raw entries
/// (virtual packages without a friendly slot, or with an off-shape value
/// the friendly form can't represent) keep their `__name` form. Subdir
/// defaults are filtered out, mirroring the on-disk shape -- only entries
/// the user actually customised appear.
pub fn inline_virtual_package_specs(
    subdir: Platform,
    declared: &[GenericVirtualPackage],
) -> Vec<InlineVirtualPackage> {
    let by_name: std::collections::HashMap<&str, &GenericVirtualPackage> = declared
        .iter()
        .map(|gvp| (gvp.name.as_normalized(), gvp))
        .collect();
    let (friendly, raw) = classify_virtual_packages(subdir, declared);
    let mut out = Vec::with_capacity(friendly.len() + raw.len());
    for (key, value) in friendly {
        // `classify_virtual_packages` filters defaults and only keeps
        // entries it could resolve to a friendly slot; the corresponding
        // `__conda_name` must therefore be in `declared`.
        let conda_name = FRIENDLY_VIRTUAL_PACKAGES
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.conda_name)
            .expect("friendly entry comes from FRIENDLY_VIRTUAL_PACKAGES");
        let package = (*by_name
            .get(conda_name)
            .expect("friendly entry came from `declared`"))
        .clone();
        out.push(InlineVirtualPackage {
            package,
            rendered: format!("{key}={value}"),
        });
    }
    for (conda_name, value) in raw {
        let package = (*by_name
            .get(conda_name.as_str())
            .expect("raw entry came from `declared`"))
        .clone();
        out.push(InlineVirtualPackage {
            package,
            rendered: format!("{conda_name}={value}"),
        });
    }
    out
}

/// Build the canonical auto-derived name for `(subdir, declared)`.
///
/// The form is `<subdir>[-<key>-<value>...]`, with friendly keys emitted in
/// the order they appear in [`FRIENDLY_VIRTUAL_PACKAGES`] and any raw
/// `__name` packages appended alphabetically. Values are sanitized so the
/// result still passes [`PixiPlatformName::try_from`] (non-alphanumeric
/// characters collapse to a single `-` and leading/trailing dashes are
/// stripped).
pub(crate) fn synthesize_name_string(
    subdir: Platform,
    declared: &[GenericVirtualPackage],
) -> String {
    let (friendly, raw) = classify_virtual_packages(subdir, declared);
    let mut parts: Vec<String> = vec![subdir.as_str().to_string()];
    for (key, value) in friendly {
        parts.push(format!("{key}-{}", sanitize_name_segment(&value)));
    }
    for (key, value) in raw {
        let stripped = key.trim_start_matches('_');
        let key_seg = sanitize_name_segment(stripped);
        let val_seg = sanitize_name_segment(&value);
        parts.push(format!("{key_seg}-{val_seg}"));
    }
    parts.join("-")
}

fn sanitize_name_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    out
}

/// `(friendly_key, value)` pair: the friendly key is one of
/// [`FRIENDLY_VIRTUAL_PACKAGES`] (a `&'static str`), the value is the
/// rendered string a user would type after the `=`.
type FriendlyEntry = (&'static str, String);

/// `(conda_name, value)` pair: the conda name is the raw `__name` form, the
/// value is `version[=build_string]`.
type RawEntry = (String, String);

/// Classify each declared virtual package into either a friendly
/// `(key, value)` entry (using the shortcut form like `cuda = "12.0"`) or a
/// raw entry that keeps the `__name` conda virtual-package name verbatim
/// because its shape doesn't fit any friendly form. Friendly entries come
/// out in canonical [`FRIENDLY_VIRTUAL_PACKAGES`] order so the serialized
/// table and the auto-derived name are stable; raw entries are sorted
/// alphabetically by conda name.
///
/// Virtual packages whose value matches the subdir default
/// (`is_subdir_default`) are filtered out so that materialised defaults
/// don't leak into the on-disk shape or the synthesised platform name.
fn classify_virtual_packages(
    subdir: Platform,
    declared: &[GenericVirtualPackage],
) -> (Vec<FriendlyEntry>, Vec<RawEntry>) {
    let customised: Vec<&GenericVirtualPackage> = declared
        .iter()
        .filter(|gvp| !is_subdir_default(gvp, subdir))
        .collect();

    let mut friendly = Vec::new();
    let mut consumed: HashSet<&str> = HashSet::new();

    for entry in FRIENDLY_VIRTUAL_PACKAGES {
        let Some(package) = customised
            .iter()
            .find(|p| p.name.as_normalized() == entry.conda_name)
        else {
            continue;
        };
        let fits = match entry.kind {
            VirtualPackageValueKind::Version => {
                package.build_string.is_empty() || package.build_string == "0"
            }
            VirtualPackageValueKind::Microarch => {
                package.version == Version::major(0) && !package.build_string.is_empty()
            }
        };
        if !fits {
            // Odd shape: don't take the friendly slot; fall through to raw.
            continue;
        }
        let value = match entry.kind {
            VirtualPackageValueKind::Version => package.version.to_string(),
            VirtualPackageValueKind::Microarch => package.build_string.clone(),
        };
        consumed.insert(entry.conda_name);
        friendly.push((entry.key, value));
    }

    let mut leftover: Vec<&GenericVirtualPackage> = customised
        .iter()
        .filter(|p| !consumed.contains(p.name.as_normalized()))
        .copied()
        .collect();
    leftover.sort_by(|a, b| a.name.as_normalized().cmp(b.name.as_normalized()));

    let raw: Vec<RawEntry> = leftover
        .into_iter()
        .map(|package| {
            let build_is_zero = package.build_string.is_empty() || package.build_string == "0";
            let value = if build_is_zero {
                package.version.to_string()
            } else {
                format!("{}={}", package.version, package.build_string)
            };
            (package.name.as_normalized().to_string(), value)
        })
        .collect();

    (friendly, raw)
}

/// Render a [`PixiPlatform`] as a [`toml_edit::Value`] using the same
/// bare-string vs inline-table shape as the serde `Serialize` impl above.
/// This lets the document-editor rewrite the `platforms` array without
/// going through serde.
pub(crate) fn pixi_platform_to_toml_value(platform: &PixiPlatform) -> toml_edit::Value {
    let name = platform.name().as_str();
    let subdir_str = platform.subdir().to_string();
    let declared = platform.declared_virtual_packages();

    let (friendly, raw) = classify_virtual_packages(platform.subdir(), declared);
    if name == subdir_str && friendly.is_empty() && raw.is_empty() {
        return toml_edit::Value::from(name);
    }

    let auto_name = synthesize_name_string(platform.subdir(), declared);

    let mut table = toml_edit::InlineTable::new();
    if name != auto_name {
        table.insert("name", name.into());
    }
    table.insert("platform", subdir_str.into());
    for (key, value) in friendly {
        table.insert(key, value.into());
    }
    for (key, value) in raw {
        table.insert(&key, value.into());
    }
    toml_edit::Value::InlineTable(table)
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

    fn virtual_package_specs(platform: &PixiPlatform) -> Vec<String> {
        platform
            .declared_virtual_packages()
            .iter()
            .map(|package| {
                let build_is_zero = package.build_string.is_empty() || package.build_string == "0";
                if build_is_zero {
                    format!("{}={}", package.name.as_normalized(), package.version)
                } else {
                    format!(
                        "{}={}={}",
                        package.name.as_normalized(),
                        package.version,
                        package.build_string
                    )
                }
            })
            .collect()
    }

    #[test]
    fn test_workspace_platform_bare_string() {
        // The bare-string form parses to a subdir platform with the subdir
        // defaults materialised.
        let parsed = TopLevel::from_toml_str(r#"platform = "linux-64""#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "linux-64");
        assert_eq!(parsed.platform.subdir(), Platform::Linux64);
        assert!(parsed.platform.is_subdir_platform());
        assert_eq!(
            parsed.platform.declared_virtual_packages(),
            crate::PixiPlatform::from_subdir(Platform::Linux64).declared_virtual_packages(),
        );
    }

    /// Bare subdir as `name`: same outcome as the bare-string form -- the
    /// platform is a subdir platform carrying the materialised defaults.
    #[test]
    fn test_workspace_platform_name_only_is_subdir() {
        let parsed = TopLevel::from_toml_str(r#"platform = { name = "osx-arm64" }"#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "osx-arm64");
        assert_eq!(parsed.platform.subdir(), Platform::OsxArm64);
        assert!(parsed.platform.is_subdir_platform());
        assert_eq!(
            parsed.platform.declared_virtual_packages(),
            crate::PixiPlatform::from_subdir(Platform::OsxArm64).declared_virtual_packages(),
        );
    }

    /// `platform` alone: subdir taken from the value, name auto-derived to the
    /// same string, no VPs.
    #[test]
    fn test_workspace_platform_only_platform_key() {
        let parsed = TopLevel::from_toml_str(r#"platform = { platform = "linux-64" }"#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "linux-64");
        assert_eq!(parsed.platform.subdir(), Platform::Linux64);
        assert!(parsed.platform.is_subdir_platform());
    }

    #[test]
    fn test_workspace_platform_friendly_virtual_packages_auto_name() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", cuda = "12.0", libc = "2.28" }"#,
        )
        .unwrap();
        assert_eq!(parsed.platform.subdir(), Platform::Linux64);
        assert_eq!(
            virtual_package_specs(&parsed.platform),
            vec![
                "__cuda=12.0".to_string(),
                "__glibc=2.28".to_string(),
                "__unix=0".to_string(),
                "__linux=4.18".to_string(),
                "__archspec=0=x86_64".to_string(),
            ]
        );
        // `libc = "2.28"` matches the linux-64 default and is elided from the
        // synthesised name, leaving only the truly customised `cuda`.
        assert_eq!(parsed.platform.name().as_str(), "linux-64-cuda-12-0");
    }

    #[test]
    fn test_workspace_platform_archspec_goes_to_build_string() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", archspec = "x86-64-v3" }"#,
        )
        .unwrap();
        let package = &parsed.platform.declared_virtual_packages()[0];
        assert_eq!(package.name.as_normalized(), "__archspec");
        assert_eq!(package.version, Version::major(0));
        assert_eq!(package.build_string, "x86-64-v3");
        assert_eq!(
            parsed.platform.name().as_str(),
            "linux-64-archspec-x86-64-v3"
        );
    }

    /// Friendly key order in the TOML source must not affect the auto-derived
    /// name: it follows the canonical order from `FRIENDLY_VIRTUAL_PACKAGES`.
    #[test]
    fn test_workspace_platform_friendly_virtual_packages_order_independent() {
        let a = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", cuda = "12.0", libc = "2.28" }"#,
        )
        .unwrap();
        let b = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", libc = "2.28", cuda = "12.0" }"#,
        )
        .unwrap();
        assert_eq!(a.platform.name(), b.platform.name());
        assert_eq!(
            virtual_package_specs(&a.platform),
            virtual_package_specs(&b.platform)
        );
    }

    #[test]
    fn test_workspace_platform_explicit_name_overrides_auto() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { name = "jetson-nano", platform = "linux-aarch64", cuda = "12.8" }"#,
        )
        .unwrap();
        assert_eq!(parsed.platform.name().as_str(), "jetson-nano");
        assert_eq!(parsed.platform.subdir(), Platform::LinuxAarch64);
        assert_eq!(
            virtual_package_specs(&parsed.platform),
            vec![
                "__cuda=12.8".to_string(),
                "__unix=0".to_string(),
                "__linux=4.18".to_string(),
                "__glibc=2.28".to_string(),
                "__archspec=0=aarch64".to_string(),
            ]
        );
    }

    /// `osx` is accepted as an alias for the `macos` friendly key, and the
    /// canonical `macos` spelling is used when serializing back.
    #[test]
    fn test_workspace_platform_osx_alias_for_macos() {
        let via_osx =
            TopLevel::from_toml_str(r#"platform = { platform = "osx-arm64", osx = "13.5" }"#)
                .unwrap();
        let via_macos =
            TopLevel::from_toml_str(r#"platform = { platform = "osx-arm64", macos = "13.5" }"#)
                .unwrap();
        assert_eq!(via_osx.platform.name(), via_macos.platform.name());
        assert_eq!(
            virtual_package_specs(&via_osx.platform),
            virtual_package_specs(&via_macos.platform),
        );
        let json = serde_json::to_value(TomlPixiPlatform(via_osx.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "platform": "osx-arm64", "macos": "13.5" }),
        );
    }

    /// Declaring both `macos` and its `osx` alias on one entry is rejected.
    #[test]
    fn test_workspace_platform_osx_and_macos_conflict() {
        let input = r#"platform = { platform = "osx-arm64", macos = "13.5", osx = "14.0" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("'osx' is an alias for 'macos'"),
            "expected alias-conflict error, got: {rendered}",
        );
    }

    /// Unknown `__<name>` entries (those we don't have a friendly shortcut
    /// for) keep working so the TOML layer doesn't need updating every time
    /// rattler learns about a new virtual package.
    #[test]
    fn test_workspace_platform_raw_virtual_package_forward_compat() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", __future_pkg = "1.2" }"#,
        )
        .unwrap();
        assert_eq!(
            virtual_package_specs(&parsed.platform),
            vec![
                "__future_pkg=1.2".to_string(),
                "__unix=0".to_string(),
                "__linux=4.18".to_string(),
                "__glibc=2.28".to_string(),
                "__archspec=0=x86_64".to_string(),
            ]
        );
        assert_eq!(parsed.platform.name().as_str(), "linux-64-future-pkg-1-2");
    }

    #[test]
    fn test_workspace_platform_invalid_name() {
        let input = r#"platform = { name = "bad name", platform = "linux-64" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, error), @r#"
         × 'bad name' is not a valid platform name (allowed: alphanumeric, '-')
          ╭─[pixi.toml:1:22]
        1 │ platform = { name = "bad name", platform = "linux-64" }
          ·                      ────────
          ╰────
        "#);
    }

    #[test]
    fn test_workspace_platform_custom_name_without_platform() {
        let input = r#"platform = { name = "linux-64-cuda" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        assert_snapshot!(format_parse_error(input, error), @r#"
         × 'linux-64-cuda' is not a conda subdir; set 'platform' explicitly when using a custom name
          ╭─[pixi.toml:1:22]
        1 │ platform = { name = "linux-64-cuda" }
          ·                      ─────────────
          ╰────
        "#);
    }

    #[test]
    fn test_workspace_platform_empty_table_rejected() {
        let input = r#"platform = {}"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("must set at least one of 'name' or 'platform'"),
            "expected error to mention required keys, got: {rendered}",
        );
    }

    #[test]
    fn test_workspace_platform_unknown_subdir() {
        let input = r#"platform = "bogus-platform""#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("bogus-platform"),
            "expected error to mention the bad subdir, got: {rendered}",
        );
    }

    #[test]
    fn test_workspace_platform_unknown_key_rejected() {
        let input = r#"platform = { platform = "linux-64", cuda = "12.0", typo = "x" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("typo"),
            "expected error to mention the unknown key 'typo', got: {rendered}",
        );
    }

    #[test]
    fn test_workspace_platform_archspec_requires_value() {
        let input = r#"platform = { platform = "linux-64", archspec = "" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("'archspec' requires a non-empty microarchitecture string"),
            "expected archspec emptiness error, got: {rendered}",
        );
    }

    /// Bad version strings on a friendly key surface the conda-version parse
    /// error (rather than dropping the cause silently or pointing at the wrong
    /// span). Conda versions are very permissive, so the input here uses
    /// characters that the version grammar genuinely rejects.
    #[test]
    fn test_workspace_platform_friendly_key_invalid_version_rejected() {
        let input = r#"platform = { platform = "linux-64", cuda = "@@@" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("'@@@' is not a valid version"),
            "expected friendly-key version error, got: {rendered}",
        );
    }

    fn platform_with_packages(
        name: &str,
        subdir: Platform,
        declared: Vec<GenericVirtualPackage>,
    ) -> PixiPlatform {
        // A subdir-named entry with no user declarations is the
        // subdir-platform shape; construct it via `from_subdir` so the
        // materialised defaults end up in the declared list.
        if name == subdir.as_str() && declared.is_empty() {
            return PixiPlatform::from_subdir(subdir);
        }
        PixiPlatform::new(
            PixiPlatformName::try_from(name).expect("valid platform name"),
            subdir,
            declared,
        )
        .expect("test inputs respect the subdir-platform invariant")
    }

    fn version_virtual_package(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: String::new(),
        }
    }

    fn archspec_virtual_package(microarch: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from("__archspec").unwrap(),
            version: Version::major(0),
            build_string: microarch.to_string(),
        }
    }

    #[test]
    fn test_serialize_bare_string() {
        let platform = platform_with_packages("linux-64", Platform::Linux64, Vec::new());
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(json, serde_json::Value::String("linux-64".into()));
    }

    #[test]
    fn test_serialize_auto_named_omits_name() {
        let platform = platform_with_packages(
            "linux-64-cuda-12-0",
            Platform::Linux64,
            vec![version_virtual_package("__cuda", "12.0")],
        );
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "cuda": "12.0",
            }),
        );
    }

    #[test]
    fn test_serialize_explicit_name_emitted() {
        let platform = platform_with_packages(
            "jetson-nano",
            Platform::LinuxAarch64,
            vec![version_virtual_package("__cuda", "12.8")],
        );
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "jetson-nano",
                "platform": "linux-aarch64",
                "cuda": "12.8",
            }),
        );
    }

    #[test]
    fn test_serialize_archspec_uses_build_string() {
        let platform = platform_with_packages(
            "linux-64-archspec-x86-64-v3",
            Platform::Linux64,
            vec![archspec_virtual_package("x86-64-v3")],
        );
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "archspec": "x86-64-v3",
            }),
        );
    }

    /// VPs whose shape doesn't match the friendly form (e.g. a `__cuda` with
    /// a non-empty build string) fall through to the raw `__name = ...` form
    /// so we never silently drop information.
    #[test]
    fn test_serialize_falls_back_to_raw_for_odd_shapes() {
        let mut odd = version_virtual_package("__cuda", "12.0");
        odd.build_string = "weird".to_string();
        let platform =
            platform_with_packages("linux-64-cuda-12-0-weird", Platform::Linux64, vec![odd]);
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "__cuda": "12.0=weird",
            }),
        );
    }

    #[test]
    fn test_roundtrip_friendly_table() {
        // `libc = "2.28"` is exactly the linux-64 default, so it's elided
        // from the serialised shape (the next parse will re-materialise it
        // from defaults). `cuda` has no default and survives the round-trip.
        let original = r#"platform = { platform = "linux-64", cuda = "12.0", libc = "2.28" }"#;
        let parsed = TopLevel::from_toml_str(original).unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "cuda": "12.0",
            }),
        );
    }

    #[test]
    fn test_roundtrip_named_table() {
        let original =
            r#"platform = { name = "jetson", platform = "linux-aarch64", cuda = "12.8" }"#;
        let parsed = TopLevel::from_toml_str(original).unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "jetson",
                "platform": "linux-aarch64",
                "cuda": "12.8",
            }),
        );
    }

    #[test]
    fn test_roundtrip_bare_string() {
        let parsed = TopLevel::from_toml_str(r#"platform = "linux-64""#).unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(json, serde_json::Value::String("linux-64".into()));
    }

    #[test]
    fn test_sanitize_name_segment_examples() {
        assert_eq!(sanitize_name_segment("12.0"), "12-0");
        assert_eq!(sanitize_name_segment("x86-64-v3"), "x86-64-v3");
        assert_eq!(sanitize_name_segment("1.2.3"), "1-2-3");
        assert_eq!(sanitize_name_segment("..."), "");
        assert_eq!(sanitize_name_segment("-leading"), "leading");
        assert_eq!(sanitize_name_segment("trailing-"), "trailing");
    }
}
