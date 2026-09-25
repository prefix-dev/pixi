use std::{collections::HashSet, str::FromStr};

use indexmap::IndexSet;
use itertools::Itertools;

use pixi_toml::TomlEnum;
use rattler_conda_types::{
    GenericVirtualPackage, PackageName, Subdir, Version,
    virtual_package_detector::VirtualPackageName,
};
use serde::{Serialize, ser::SerializeMap};
use toml_span::{
    DeserError, Deserialize, Error, ErrorKind, Span, Spanned, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

use crate::{
    PixiPlatform, PixiPlatformName,
    platform::subdir_default_virtual_packages,
    system_requirements::{SystemRequirements, virtual_packages_for_subdir},
};

/// This type is used to represent the platform in the manifest file. The
/// [`Subdir`] type from rattler contains more platforms than we actually
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

impl From<TomlPlatform> for Subdir {
    fn from(value: TomlPlatform) -> Self {
        match value {
            TomlPlatform::Linux32 => Subdir::Linux32,
            TomlPlatform::Linux64 => Subdir::Linux64,
            TomlPlatform::LinuxAarch64 => Subdir::LinuxAarch64,
            TomlPlatform::LinuxArmv6l => Subdir::LinuxArmV6l,
            TomlPlatform::LinuxArmv7l => Subdir::LinuxArmV7l,
            TomlPlatform::LinuxPpc64le => Subdir::LinuxPpc64le,
            TomlPlatform::LinuxPpc64 => Subdir::LinuxPpc64,
            TomlPlatform::LinuxS390X => Subdir::LinuxS390X,
            TomlPlatform::LinuxRiscv32 => Subdir::LinuxRiscv32,
            TomlPlatform::LinuxRiscv64 => Subdir::LinuxRiscv64,
            TomlPlatform::Osx64 => Subdir::Osx64,
            TomlPlatform::OsxArm64 => Subdir::OsxArm64,
            TomlPlatform::Win32 => Subdir::Win32,
            TomlPlatform::Win64 => Subdir::Win64,
            TomlPlatform::WinArm64 => Subdir::WinArm64,
            TomlPlatform::EmscriptenWasm32 => Subdir::EmscriptenWasm32,
            TomlPlatform::WasiWasm32 => Subdir::WasiWasm32,
            TomlPlatform::ZosZ => Subdir::ZosZ,
        }
    }
}

impl<'de> Deserialize<'de> for TomlPlatform {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        TomlEnum::deserialize(value).map(TomlEnum::into_inner)
    }
}

impl<'de> pixi_toml::DeserializeAs<'de, Subdir> for TomlPlatform {
    fn deserialize_as(value: &mut Value<'de>) -> Result<Subdir, DeserError> {
        TomlPlatform::deserialize(value).map(Subdir::from)
    }
}

/// How a friendly virtual-package key's value is mapped onto a
/// [`GenericVirtualPackage`].
#[derive(Debug, Clone, Copy)]
enum VirtualPackageValueKind {
    /// The value is a version string with an optional build string and lands
    /// in the corresponding `GenericVirtualPackage` fields.
    Version,
    /// The value is a microarchitecture string, optionally prefixed by
    /// `version=`. The microarchitecture lands in `build_string`; plain names
    /// default to version `0`.
    Microarch,
}

/// A friendly manifest shortcut for a virtual package.
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
        key: "glibc",
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
///   { platform = "linux-64", cuda = "12.0", glibc = "2.28" },
///   { name = "gpu", platform = "linux-64", cuda = { driver = "12.0", arch = "8.6" } },
///   { name = "jetson", platform = "linux-aarch64", cuda = "12.8", archspec = "armv8.2a" },
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
/// * Each remaining key declares a virtual package. Pixi adds the canonical
///   `__` prefix, so `amdgpu = "0"` declares `__amdgpu=0`. Values use
///   `"version"` or `"version=build_string"`. This keeps the manifest
///   forward-compatible with detector-provided names Pixi did not know when it
///   was compiled.
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
                let subdir = Subdir::from_str(&s).map_err(|e| Error {
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
                // `cuda` is the one friendly key with a nested-table form, so it
                // is parsed before the generic scalar-only loop (which then
                // skips it -- the key has already been consumed).
                declared.extend(take_cuda_entry(&mut th)?);
                for entry in FRIENDLY_VIRTUAL_PACKAGES {
                    if let Some(raw) = take_friendly_value(&mut th, entry)? {
                        declared.push(build_friendly_virtual_package(
                            entry.conda_name,
                            entry.kind,
                            &raw,
                        )?);
                    }
                }

                // Every remaining key is a detector-compatible virtual package.
                // The manifest omits the canonical `__` prefix.
                let custom_keys: Vec<String> = th
                    .table
                    .keys()
                    .map(|key| key.name.as_ref().to_owned())
                    .collect();
                for key_name in custom_keys {
                    let (key, mut entry_value) = th
                        .table
                        .remove_entry(key_name.as_str())
                        .expect("just enumerated");
                    let gvp = parse_custom_virtual_package(
                        key.name.as_ref(),
                        key.span,
                        &mut entry_value,
                    )?;
                    if declared.iter().any(|declared| declared.name == gvp.name) {
                        return Err(Error {
                            kind: ErrorKind::Custom(
                                format!(
                                    "'{}' is declared more than once",
                                    gvp.name.as_normalized(),
                                )
                                .into(),
                            ),
                            span: key.span,
                            line_info: None,
                        }
                        .into());
                    }
                    declared.push(gvp);
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
                        // The subdir-platform error has an actionable fix
                        // (drop the VPs or rename); the coupling error speaks
                        // for itself, so don't bolt the rename hint onto it.
                        let message = match e {
                            crate::platform::PixiPlatformError::IsSubdirPlatform => format!(
                                "platform entry rejected: {e}; either drop the virtual packages or give the entry a `name` distinct from its subdir",
                            ),
                            other => other.to_string(),
                        };
                        Error {
                            kind: ErrorKind::Custom(message.into()),
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

        let entries = platform_inline_entries(
            declared,
            Some(&subdir_default_virtual_packages(platform.subdir())),
        );
        // Subdir platforms (`name == subdir`) carry the subdir defaults, but
        // the defaults are filtered out by `platform_inline_entries` so
        // `entries` ends up empty -- the bare-string shape covers them exactly.
        // Falls through to the inline-table shape as soon as there is a
        // non-default virtual package or a custom name.
        if name == subdir_str && entries.is_empty() {
            return serializer.serialize_str(name);
        }

        let emit_name = !platform.has_derived_name();

        let count = 1 + usize::from(emit_name) + entries.len();
        let mut map = serializer.serialize_map(Some(count))?;
        if emit_name {
            map.serialize_entry("name", name)?;
        }
        map.serialize_entry("platform", &subdir_str)?;
        for entry in &entries {
            match entry {
                InlinePlatformEntry::Scalar { key, value, .. } => {
                    map.serialize_entry(key, value)?;
                }
                InlinePlatformEntry::CudaTable { driver, arch } => {
                    map.serialize_entry(
                        "cuda",
                        &CudaTableRepr {
                            driver: driver.as_str(),
                            arch: arch.as_str(),
                        },
                    )?;
                }
            }
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
            let (version, build_string) = raw
                .value
                .split_once('=')
                .map_or((raw.value.as_str(), ""), |(version, build)| {
                    (version, build)
                });
            let version = Version::from_str(version).map_err(|e| Error {
                kind: ErrorKind::Custom(
                    format!(
                        "'{}' is not a valid version and build string: {e}",
                        raw.value
                    )
                    .into(),
                ),
                span: raw.span,
                line_info: None,
            })?;
            crate::platform::validate_virtual_package_build_string(&package_name, build_string)
                .map_err(|message| Error {
                    kind: ErrorKind::Custom(message.into()),
                    span: raw.span,
                    line_info: None,
                })?;
            Ok(GenericVirtualPackage {
                name: package_name,
                version,
                build_string: build_string.to_string(),
            })
        }
        VirtualPackageValueKind::Microarch => {
            let (version, microarch) = match raw.value.split_once('=') {
                Some((version, microarch)) => {
                    let version = Version::from_str(version).map_err(|e| Error {
                        kind: ErrorKind::Custom(
                            format!(
                                "'{}' is not a valid version and build string: {e}",
                                raw.value
                            )
                            .into(),
                        ),
                        span: raw.span,
                        line_info: None,
                    })?;
                    (version, microarch)
                }
                None => (Version::major(0), raw.value.as_str()),
            };
            if microarch.is_empty() {
                return Err(Error {
                    kind: ErrorKind::Custom(
                        "'archspec' requires a non-empty microarchitecture string".into(),
                    ),
                    span: raw.span,
                    line_info: None,
                });
            }
            crate::platform::validate_archspec_name(microarch).map_err(|message| Error {
                kind: ErrorKind::Custom(message.into()),
                span: raw.span,
                line_info: None,
            })?;
            Ok(GenericVirtualPackage {
                name: package_name,
                version,
                build_string: microarch.to_string(),
            })
        }
    }
}

/// Parse the optional `cuda` key, which is the one friendly key that accepts a
/// nested table. Either a bare version string (`cuda = "12.0"` -> `__cuda`) or
/// a `{ driver, arch }` table (`driver` -> `__cuda`, optional `arch` ->
/// `__cuda_arch`). Returns the CUDA virtual packages in canonical order
/// (driver before arch).
///
/// Enforces the CEP coupling at parse time: `arch` without `driver` is
/// rejected, as is an empty `cuda = {}` table. Unknown inner keys (e.g. a
/// `drivers` typo) are rejected by the nested `finalize`.
fn take_cuda_entry<'de>(
    th: &mut TableHelper<'de>,
) -> Result<Vec<GenericVirtualPackage>, DeserError> {
    let Some((_key, mut value)) = th.table.remove_entry("cuda") else {
        return Ok(Vec::new());
    };
    let span = value.span;
    match value.take() {
        ValueInner::String(s) => {
            let driver = Spanned {
                value: s.into_owned(),
                span,
            };
            Ok(vec![build_friendly_virtual_package(
                "__cuda",
                VirtualPackageValueKind::Version,
                &driver,
            )?])
        }
        inner @ ValueInner::Table(_) => {
            let mut inner_value = Value::with_span(inner, span);
            let mut inner_th = TableHelper::new(&mut inner_value)?;
            let driver: Option<Spanned<String>> = inner_th.optional("driver");
            let arch: Option<Spanned<String>> = inner_th.optional("arch");
            inner_th.finalize(None)?;
            match (driver, arch) {
                (Some(driver), arch) => {
                    let mut out = vec![build_friendly_virtual_package(
                        "__cuda",
                        VirtualPackageValueKind::Version,
                        &driver,
                    )?];
                    if let Some(arch) = arch {
                        out.push(build_friendly_virtual_package(
                            "__cuda_arch",
                            VirtualPackageValueKind::Version,
                            &arch,
                        )?);
                    }
                    Ok(out)
                }
                (None, Some(arch)) => Err(Error {
                    kind: ErrorKind::Custom(
                        "`cuda.arch` requires `cuda.driver`: `__cuda_arch` is only valid alongside `__cuda`".into(),
                    ),
                    span: arch.span,
                    line_info: None,
                }
                .into()),
                (None, None) => Err(Error {
                    kind: ErrorKind::Custom(
                        "`cuda` table must set `driver` (and optionally `arch`)".into(),
                    ),
                    span,
                    line_info: None,
                }
                .into()),
            }
        }
        other => Err(expected("a version string or a table", other, span).into()),
    }
}

/// Parse `name = "version[=build_string]"` into the canonical `__name`
/// [`GenericVirtualPackage`]. Detector protocol validation keeps accepted
/// manifest keys aligned with names a detector may report.
fn parse_custom_virtual_package(
    key: &str,
    key_span: Span,
    value: &mut Value<'_>,
) -> Result<GenericVirtualPackage, Error> {
    if key.starts_with("__") {
        return Err(Error {
            kind: ErrorKind::Custom(
                format!("virtual-package key '{key}' must omit the '__' prefix").into(),
            ),
            span: key_span,
            line_info: None,
        });
    }
    let conda_name = format!("__{key}");
    let name = VirtualPackageName::try_from(conda_name)
        .map_err(|error| Error {
            kind: ErrorKind::Custom(
                format!("'{key}' is not a valid virtual-package key: {error}").into(),
            ),
            span: key_span,
            line_info: None,
        })?
        .into_package_name();
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
    crate::platform::validate_virtual_package_build_string(&name, &build_string).map_err(
        |message| Error {
            kind: ErrorKind::Custom(message.into()),
            span: value_span,
            line_info: None,
        },
    )?;
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
) -> Result<Subdir, DeserError> {
    if let Some(p) = platform_value {
        return Subdir::from_str(&p.value).map_err(|e| {
            Error {
                kind: ErrorKind::Custom(e.to_string().into()),
                span: p.span,
                line_info: None,
            }
            .into()
        });
    }
    if let Some(n) = name_value {
        return Subdir::from_str(&n.value).map_err(|_| {
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
    subdir: Subdir,
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
/// Pairs the rendered `key=value` text, without the canonical `__` prefix,
/// with the underlying [`GenericVirtualPackage`] the entry came from. The CLI
/// uses the latter to do identity and satisfaction checks against host-detected
/// packages without re-parsing the rendered form.
#[derive(Debug, Clone)]
pub struct InlineVirtualPackage {
    /// The conda virtual package(s) the entry represents. Usually one, but the
    /// grouped `cuda = { driver, arch }` entry carries both `__cuda` and
    /// `__cuda_arch` so callers can satisfaction-check each.
    pub packages: Vec<GenericVirtualPackage>,
    /// On-line rendering. Built-in shortcuts use their friendly keys and
    /// arbitrary packages omit the canonical `__` prefix. Coupled CUDA
    /// packages render as `cuda = { driver = "..", arch = ".." }`.
    pub rendered: String,
}

/// Render a platform's declared virtual packages as the inline `key=value`
/// strings used in `pixi.toml` and `pixi workspace platform add`, paired
/// with the underlying conda VP so callers can run match logic against
/// them.
///
/// Built-in entries use their friendly short keys in
/// canonical order. Arbitrary entries strip their canonical `__` prefix and
/// sort alphabetically. Subdir defaults are filtered out, mirroring the
/// on-disk shape, so only entries the user customised appear.
pub fn inline_virtual_package_specs(
    declared: &[GenericVirtualPackage],
    baseline: Option<&[GenericVirtualPackage]>,
) -> Vec<InlineVirtualPackage> {
    let by_name: std::collections::HashMap<&str, &GenericVirtualPackage> = declared
        .iter()
        .map(|gvp| (gvp.name.as_normalized(), gvp))
        .collect();
    let lookup =
        |conda_name: &str| (*by_name.get(conda_name).expect("entry came from `declared`")).clone();
    platform_inline_entries(declared, baseline)
        .into_iter()
        .map(|entry| match entry {
            InlinePlatformEntry::Scalar {
                key,
                value,
                conda_name,
            } => InlineVirtualPackage {
                packages: vec![lookup(&conda_name)],
                rendered: render_key_value(&key, &value),
            },
            InlinePlatformEntry::CudaTable { driver, arch } => InlineVirtualPackage {
                packages: vec![lookup("__cuda"), lookup("__cuda_arch")],
                rendered: render_cuda_table(&driver, &arch),
            },
        })
        .collect()
}

/// Render the grouped CUDA inline table verbatim as it appears in `pixi.toml`,
/// so the `list`/`info` display matches the on-disk shape exactly.
fn render_cuda_table(driver: &str, arch: &str) -> String {
    format!("cuda = {{ driver = \"{driver}\", arch = \"{arch}\" }}")
}

/// Render a classified `key`/`value` pair. A version-0 entry (`value == "0"`)
/// renders as just the unprefixed key (`amdgpu`, `glibc`); otherwise `key=value`.
fn render_key_value(key: &str, value: &str) -> String {
    if value == "0" {
        key.to_string()
    } else {
        format!("{key}={value}")
    }
}

/// Build the canonical auto-derived name for `(subdir, declared)`.
///
/// The form is `<subdir>[-<key>-<value>...]`, with built-in keys emitted in
/// the order they appear in [`FRIENDLY_VIRTUAL_PACKAGES`] and all other keys
/// appended alphabetically. Values are sanitized so the result still passes
/// [`PixiPlatformName::try_from`] (non-alphanumeric characters collapse to a
/// single `-` and leading/trailing dashes are stripped).
pub(crate) fn synthesize_name_string(subdir: Subdir, declared: &[GenericVirtualPackage]) -> String {
    let (friendly, raw) =
        classify_virtual_packages(declared, Some(&subdir_default_virtual_packages(subdir)));
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

/// `(conda_name, value)` pair for a package without a built-in shortcut. The
/// conda name retains its canonical `__` prefix internally; renderers remove
/// it at the manifest and platform-display boundary.
type RawEntry = (String, String);

/// Classify each declared virtual package into either a built-in shortcut
/// `(key, value)` entry or an arbitrary entry. Built-in entries follow
/// [`FRIENDLY_VIRTUAL_PACKAGES`] order and arbitrary entries sort by canonical
/// conda name.
///
/// Virtual packages whose value matches the subdir default
/// (`is_subdir_default`) are filtered out so that materialised defaults
/// don't leak into the on-disk shape or the synthesised platform name.
fn classify_virtual_packages(
    declared: &[GenericVirtualPackage],
    baseline: Option<&[GenericVirtualPackage]>,
) -> (Vec<FriendlyEntry>, Vec<RawEntry>) {
    let customised: Vec<&GenericVirtualPackage> = declared
        .iter()
        .filter(|gvp| {
            baseline.is_none_or(|base| {
                !base.iter().any(|d| {
                    d.name == gvp.name
                        && d.version == gvp.version
                        && d.build_string == gvp.build_string
                })
            })
        })
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

/// A declared virtual package in its friendly on-disk/on-line form, after the
/// coupled CUDA packages are grouped. Every renderer (TOML serialize, the
/// document editor, the `key=value` list/`info` display) goes through
/// [`platform_inline_entries`] so the `cuda` grouping lives in exactly one
/// place.
enum InlinePlatformEntry {
    /// A flat entry using an unprefixed manifest key.
    /// `conda_name` is the canonical virtual-package name it represents.
    Scalar {
        key: String,
        value: String,
        conda_name: String,
    },
    /// The grouped CUDA table `cuda = { driver, arch }`. Only emitted when both
    /// `__cuda` and `__cuda_arch` are present.
    CudaTable { driver: String, arch: String },
}

/// Serde shape for the nested `cuda = { driver, arch }` table.
#[derive(Serialize)]
struct CudaTableRepr<'a> {
    driver: &'a str,
    arch: &'a str,
}

/// Classify `declared` into ordered friendly entries, grouping `__cuda` +
/// `__cuda_arch` into a single [`InlinePlatformEntry::CudaTable`].
///
/// Wraps [`classify_virtual_packages`] (whose flat output still drives name
/// synthesis) and only reshapes the rendering, so the auto-derived platform
/// name stays independent of the `cuda` table grouping. A lone `__cuda_arch`
/// falls through to the direct `cuda_arch` scalar.
fn platform_inline_entries(
    declared: &[GenericVirtualPackage],
    baseline: Option<&[GenericVirtualPackage]>,
) -> Vec<InlinePlatformEntry> {
    let (friendly, raw) = classify_virtual_packages(declared, baseline);
    // The arch value lives in the raw bucket (no friendly key maps to
    // `__cuda_arch`); pull it out so a `cuda` friendly entry can absorb it.
    let arch_value = raw
        .iter()
        .find(|(conda_name, _)| conda_name == "__cuda_arch")
        .map(|(_, value)| value.clone());

    let mut entries = Vec::with_capacity(friendly.len() + raw.len());
    let mut cuda_grouped = false;
    for (key, value) in friendly {
        if key == "cuda"
            && let Some(arch) = arch_value.clone()
        {
            entries.push(InlinePlatformEntry::CudaTable {
                driver: value,
                arch,
            });
            cuda_grouped = true;
        } else {
            let conda_name = FRIENDLY_VIRTUAL_PACKAGES
                .iter()
                .find(|entry| entry.key == key)
                .map(|entry| entry.conda_name.to_string())
                .expect("friendly entry comes from FRIENDLY_VIRTUAL_PACKAGES");
            entries.push(InlinePlatformEntry::Scalar {
                key: key.to_string(),
                value,
                conda_name,
            });
        }
    }
    for (conda_name, value) in raw {
        // The arch entry was folded into the `cuda` table above.
        if conda_name == "__cuda_arch" && cuda_grouped {
            continue;
        }
        let key = conda_name
            .strip_prefix("__")
            .unwrap_or(&conda_name)
            .to_string();
        entries.push(InlinePlatformEntry::Scalar {
            key,
            value,
            conda_name,
        });
    }
    entries
}

/// Render the `platforms` array that replaces a legacy `[system-requirements]`
/// table: one entry per subdir, each carrying only the requirements that apply
/// there.
///
/// A requirement equal to the subdir's default is still written out, where the
/// serializer would drop it. `subdirs` may be empty, which falls back to a
/// single `linux-64` entry.
pub(crate) fn system_requirements_as_platforms(
    sysreqs: &SystemRequirements,
    subdirs: &[Subdir],
) -> String {
    // Two platforms can share a subdir; emitting it twice would give two
    // entries the same `my-` name.
    let unique: IndexSet<Subdir> = if subdirs.is_empty() {
        IndexSet::from([Subdir::Linux64])
    } else {
        subdirs.iter().copied().collect()
    };

    let candidates = sysreqs.to_declared_virtual_packages();
    let rendered = unique
        .iter()
        .map(|&subdir| suggested_platform_entry(&candidates, subdir))
        .format(", ");
    format!("platforms = [{rendered}]")
}

/// One suggested `platforms` entry for `subdir`.
fn suggested_platform_entry(candidates: &[GenericVirtualPackage], subdir: Subdir) -> String {
    let declared = virtual_packages_for_subdir(candidates, subdir);
    let entries = platform_inline_entries(&declared, None);
    if entries.is_empty() {
        return format!("\"{subdir}\"");
    }
    let pairs = entries
        .iter()
        .map(|entry| match entry {
            InlinePlatformEntry::Scalar { key, value, .. } => format!("{key} = \"{value}\""),
            InlinePlatformEntry::CudaTable { driver, arch } => render_cuda_table(driver, arch),
        })
        .format(", ");
    // Without an explicit name, a requirement equal to the subdir default
    // collapses the name back to the bare subdir, which is then rejected as
    // `IsSubdirPlatform`.
    format!("{{ name = \"my-{subdir}\", platform = \"{subdir}\", {pairs} }}")
}

/// Render a [`PixiPlatform`] as a [`toml_edit::Value`] using the same
/// bare-string vs inline-table shape as the serde `Serialize` impl above.
/// This lets the document-editor rewrite the `platforms` array without
/// going through serde.
pub(crate) fn pixi_platform_to_toml_value(platform: &PixiPlatform) -> toml_edit::Value {
    let name = platform.name().as_str();
    let subdir_str = platform.subdir().to_string();
    let declared = platform.declared_virtual_packages();

    let entries = platform_inline_entries(
        declared,
        Some(&subdir_default_virtual_packages(platform.subdir())),
    );
    if name == subdir_str && entries.is_empty() {
        return toml_edit::Value::from(name);
    }

    let mut table = toml_edit::InlineTable::new();
    if !platform.has_derived_name() {
        table.insert("name", name.into());
    }
    table.insert("platform", subdir_str.into());
    for entry in entries {
        match entry {
            InlinePlatformEntry::Scalar { key, value, .. } => {
                table.insert(&key, value.into());
            }
            InlinePlatformEntry::CudaTable { driver, arch } => {
                let mut inner = toml_edit::InlineTable::new();
                inner.insert("driver", driver.into());
                inner.insert("arch", arch.into());
                table.insert("cuda", toml_edit::Value::InlineTable(inner));
            }
        }
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
        assert_eq!(parsed.platform.subdir(), Subdir::Linux64);
        assert!(parsed.platform.is_subdir_platform());
        assert_eq!(
            parsed.platform.declared_virtual_packages(),
            crate::PixiPlatform::from_subdir(Subdir::Linux64).declared_virtual_packages(),
        );
    }

    /// Bare subdir as `name`: same outcome as the bare-string form -- the
    /// platform is a subdir platform carrying the materialised defaults.
    #[test]
    fn test_workspace_platform_name_only_is_subdir() {
        let parsed = TopLevel::from_toml_str(r#"platform = { name = "osx-arm64" }"#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "osx-arm64");
        assert_eq!(parsed.platform.subdir(), Subdir::OsxArm64);
        assert!(parsed.platform.is_subdir_platform());
        assert_eq!(
            parsed.platform.declared_virtual_packages(),
            crate::PixiPlatform::from_subdir(Subdir::OsxArm64).declared_virtual_packages(),
        );
    }

    /// `platform` alone: subdir taken from the value, name auto-derived to the
    /// same string, no VPs.
    #[test]
    fn test_workspace_platform_only_platform_key() {
        let parsed = TopLevel::from_toml_str(r#"platform = { platform = "linux-64" }"#).unwrap();
        assert_eq!(parsed.platform.name().as_str(), "linux-64");
        assert_eq!(parsed.platform.subdir(), Subdir::Linux64);
        assert!(parsed.platform.is_subdir_platform());
    }

    #[test]
    fn test_workspace_platform_friendly_virtual_packages_auto_name() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", cuda = "12.0", glibc = "2.28" }"#,
        )
        .unwrap();
        assert_eq!(parsed.platform.subdir(), Subdir::Linux64);
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
        // `glibc = "2.28"` matches the linux-64 default and is elided from the
        // synthesised name, leaving only the truly customised `cuda`.
        assert_eq!(parsed.platform.name().as_str(), "linux-64-cuda-12-0");
    }

    #[test]
    fn test_workspace_platform_arbitrary_virtual_packages_need_no_client_support() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { name = "detected", platform = "linux-64", amdgpu = "0", site_service = "2=h1" }"#,
        )
        .unwrap();
        let custom: Vec<String> = virtual_package_specs(&parsed.platform)
            .into_iter()
            .filter(|package| {
                package.starts_with("__amdgpu") || package.starts_with("__site_service")
            })
            .collect();
        assert_eq!(custom, ["__amdgpu=0", "__site_service=2=h1"]);
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "detected",
                "platform": "linux-64",
                "amdgpu": "0",
                "site_service": "2=h1",
            }),
        );
    }

    #[test]
    fn test_workspace_platform_archspec_goes_to_build_string() {
        let parsed = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", archspec = "x86_64_v3" }"#,
        )
        .unwrap();
        let package = &parsed.platform.declared_virtual_packages()[0];
        assert_eq!(package.name.as_normalized(), "__archspec");
        assert_eq!(package.version, Version::major(0));
        assert_eq!(package.build_string, "x86_64_v3");
        // The synthesised name sanitises the underscores into dashes.
        assert_eq!(
            parsed.platform.name().as_str(),
            "linux-64-archspec-x86-64-v3"
        );
    }

    #[test]
    fn test_workspace_platform_archspec_rejects_unknown_names() {
        // The dashed spelling of a known microarchitecture gets a hint.
        let input = r#"platform = { platform = "linux-64", archspec = "x86-64-v3" }"#;
        let rendered = format_parse_error(input, TopLevel::from_toml_str(input).unwrap_err());
        assert!(
            rendered.contains("did you mean 'x86_64_v3'"),
            "expected a did-you-mean hint, got: {rendered}",
        );
        let input = r#"platform = { platform = "linux-aarch64", archspec = "armv8-a" }"#;
        let rendered = format_parse_error(input, TopLevel::from_toml_str(input).unwrap_err());
        assert!(
            rendered.contains("'armv8-a' is not a known archspec microarchitecture"),
            "expected an unknown-name error, got: {rendered}",
        );
    }

    /// Friendly key order in the TOML source must not affect the auto-derived
    /// name: it follows the canonical order from `FRIENDLY_VIRTUAL_PACKAGES`.
    #[test]
    fn test_workspace_platform_friendly_virtual_packages_order_independent() {
        let a = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", cuda = "12.0", glibc = "2.28" }"#,
        )
        .unwrap();
        let b = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", glibc = "2.28", cuda = "12.0" }"#,
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
        assert_eq!(parsed.platform.subdir(), Subdir::LinuxAarch64);
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

    /// Arbitrary detector-compatible keys work without a compiled-in shortcut.
    #[test]
    fn test_workspace_platform_arbitrary_virtual_package_forward_compat() {
        let parsed =
            TopLevel::from_toml_str(r#"platform = { platform = "linux-64", future_pkg = "1.2" }"#)
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
    fn test_workspace_platform_cuda_table_parses() {
        // The `cuda` table expands to `__cuda` + `__cuda_arch`.
        let parsed = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", cuda = { driver = "12.0", arch = "8.6" } }"#,
        )
        .unwrap();
        assert_eq!(parsed.platform.subdir(), Subdir::Linux64);
        assert_eq!(
            virtual_package_specs(&parsed.platform),
            vec![
                "__cuda=12.0".to_string(),
                "__cuda_arch=8.6".to_string(),
                "__unix=0".to_string(),
                "__linux=4.18".to_string(),
                "__glibc=2.28".to_string(),
                "__archspec=0=x86_64".to_string(),
            ]
        );
        assert_eq!(
            parsed.platform.name().as_str(),
            "linux-64-cuda-12-0-cuda-arch-8-6"
        );
    }

    /// A bare `cuda = "12.0"` is exactly equivalent to `cuda = { driver = "12.0" }`.
    #[test]
    fn test_workspace_platform_cuda_table_driver_only_equals_scalar() {
        let scalar =
            TopLevel::from_toml_str(r#"platform = { platform = "linux-64", cuda = "12.0" }"#)
                .unwrap();
        let table = TopLevel::from_toml_str(
            r#"platform = { platform = "linux-64", cuda = { driver = "12.0" } }"#,
        )
        .unwrap();
        assert_eq!(scalar.platform.name(), table.platform.name());
        assert_eq!(
            virtual_package_specs(&scalar.platform),
            virtual_package_specs(&table.platform),
        );
    }

    /// `arch` without `driver` violates the CEP coupling and is rejected.
    #[test]
    fn test_workspace_platform_cuda_arch_without_driver_rejected() {
        let input = r#"platform = { platform = "linux-64", cuda = { arch = "8.6" } }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("`cuda.arch` requires `cuda.driver`"),
            "expected coupling error, got: {rendered}",
        );
    }

    /// A lone `cuda_arch` (no `cuda` anywhere) is rejected by the model.
    #[test]
    fn test_workspace_platform_cuda_arch_without_cuda_rejected() {
        let input = r#"platform = { name = "gpu", platform = "linux-64", cuda_arch = "8.6" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("`__cuda_arch` requires `__cuda`"),
            "expected coupling error, got: {rendered}",
        );
    }

    #[test]
    fn test_workspace_platform_cuda_table_unknown_key_rejected() {
        let input =
            r#"platform = { platform = "linux-64", cuda = { driver = "12.0", drivers = "x" } }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("drivers"),
            "expected unknown-inner-key error, got: {rendered}",
        );
    }

    #[test]
    fn test_workspace_platform_cuda_table_empty_rejected() {
        let input = r#"platform = { platform = "linux-64", cuda = {} }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("`cuda` table must set `driver`"),
            "expected empty-table error, got: {rendered}",
        );
    }

    #[test]
    fn test_workspace_platform_prefixed_virtual_package_rejected() {
        let input = r#"platform = { platform = "linux-64", __amdgpu = "0" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("must omit the '__' prefix"),
            "expected prefix migration error, got: {rendered}",
        );
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
    fn test_workspace_platform_arbitrary_key_uses_detector_name_rules() {
        let input = r#"platform = { platform = "linux-64", AMDGPU = "0" }"#;
        let error = TopLevel::from_toml_str(input).unwrap_err();
        let rendered = format_parse_error(input, error);
        assert!(
            rendered.contains("not a valid virtual-package key"),
            "expected detector-name validation error, got: {rendered}",
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
        subdir: Subdir,
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
        let platform = platform_with_packages("linux-64", Subdir::Linux64, Vec::new());
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(json, serde_json::Value::String("linux-64".into()));
    }

    #[test]
    fn test_serialize_auto_named_omits_name() {
        let platform = platform_with_packages(
            "linux-64-cuda-12-0",
            Subdir::Linux64,
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
            Subdir::LinuxAarch64,
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
            Subdir::Linux64,
            vec![archspec_virtual_package("x86_64_v3")],
        );
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "archspec": "x86_64_v3",
            }),
        );
    }

    /// `__cuda` + `__cuda_arch` serialize as the grouped `cuda` table.
    #[test]
    fn test_serialize_cuda_table() {
        let platform = platform_with_packages(
            "linux-64-cuda-12-0-cuda-arch-8-6",
            Subdir::Linux64,
            vec![
                version_virtual_package("__cuda", "12.0"),
                version_virtual_package("__cuda_arch", "8.6"),
            ],
        );
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "cuda": { "driver": "12.0", "arch": "8.6" },
            }),
        );
    }

    /// `__cuda` alone still serializes as the bare scalar, not a table.
    #[test]
    fn test_serialize_cuda_driver_only_stays_scalar() {
        let platform = platform_with_packages(
            "linux-64-cuda-12-0",
            Subdir::Linux64,
            vec![version_virtual_package("__cuda", "12.0")],
        );
        let json = serde_json::to_value(TomlPixiPlatform(platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "platform": "linux-64", "cuda": "12.0" }),
        );
    }

    #[test]
    fn test_roundtrip_builtin_package_with_build_string() {
        let input = r#"platform = { platform = "linux-64", cuda = "12.0=h1" }"#;
        let parsed = TopLevel::from_toml_str(input).unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "platform": "linux-64", "cuda": "12.0=h1" })
        );
    }

    #[test]
    fn test_roundtrip_cuda_table() {
        let original =
            r#"platform = { platform = "linux-64", cuda = { driver = "12.0", arch = "8.6" } }"#;
        let parsed = TopLevel::from_toml_str(original).unwrap();
        let json = serde_json::to_value(TomlPixiPlatform(parsed.platform)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "platform": "linux-64",
                "cuda": { "driver": "12.0", "arch": "8.6" },
            }),
        );
    }

    #[test]
    fn test_roundtrip_friendly_table() {
        // `glibc = "2.28"` is exactly the linux-64 default, so it's elided
        // from the serialised shape (the next parse will re-materialise it
        // from defaults). `cuda` has no default and survives the round-trip.
        let original = r#"platform = { platform = "linux-64", cuda = "12.0", glibc = "2.28" }"#;
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
