//! The `conda-pypi-map` value type, shared between the workspace manifest
//! (`[workspace.conda-pypi-map]`) and the global configuration
//! (`default-conda-pypi-map`). Both TOML dialects are supported: toml_span
//! for the manifest parser and serde for the configuration file.

use std::collections::HashMap;

use pixi_toml::{TomlEnum, TomlHashMap, custom_error, custom_error_message_with_help};
use rattler_conda_types::NamedChannelOrUrl;
use toml_span::{
    DeserError, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

/// The value of `[workspace.conda-pypi-map]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CondaPypiMap {
    /// `conda-pypi-map = false`: disable purl derivation entirely, including
    /// the offline same-name heuristic.
    Disabled,
    /// Per-channel mapping configuration. An empty map is a soft-deprecated
    /// alias for `Disabled`.
    Map(HashMap<NamedChannelOrUrl, CondaPypiMapEntry>),
}

/// How a project-defined channel mapping interacts with the default
/// prefix.dev derivation chain.
#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    strum::Display,
    strum::VariantNames,
    strum::EnumString,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CondaPypiMappingMode {
    /// The project mapping overlays Pixi's default mapping data: project
    /// entries win, and misses fall through to the prefix.dev chain.
    #[default]
    Overlay,
    /// The project mapping replaces Pixi's default mapping data. The
    /// same-name heuristic is controlled separately.
    Replace,
}

/// The mapping configuration for one channel in `[workspace.conda-pypi-map]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CondaPypiMapEntry {
    /// `<channel> = false`: disable purl derivation for this channel.
    Disabled,
    /// A mapping defined by a location (file or URL) and/or inline entries.
    Map(CondaPypiMapSpec),
}

/// A channel mapping built from up to two sources: an external location and
/// inline entries. Inline entries override entries from the location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CondaPypiMapSpec {
    /// An external mapping JSON file: a file path or http(s) URL. Unresolved:
    /// relative paths are resolved against the workspace root by the consumer.
    pub location: Option<String>,
    /// Inline conda-name to pypi-name entries. One conda package may map to
    /// several PyPI names. An empty list (spelled `false` in TOML) means the
    /// package is not a PyPI package.
    pub mapping: Option<HashMap<String, Vec<String>>>,
    pub mapping_mode: CondaPypiMappingMode,
    /// Whether Pixi may assume the conda package name is also the PyPI name
    /// when mapping data has no answer. If unset, this defaults to true for
    /// conda-forge and false for other channels.
    pub same_name_heuristic: Option<bool>,
}

impl CondaPypiMapEntry {
    /// Create an entry from a bare location string. Bare strings use the
    /// default (overlay) mapping mode.
    pub fn from_location(location: String) -> Self {
        Self::Map(CondaPypiMapSpec {
            location: Some(location),
            mapping: None,
            mapping_mode: CondaPypiMappingMode::default(),
            same_name_heuristic: None,
        })
    }
}

/// Serde support for [`CondaPypiMap`] and its parts, mirroring the manifest's
/// toml_span grammar so the same value can live in serde-deserialized
/// configuration (e.g. the global config's `default-conda-pypi-map`):
/// `false` | `{ <channel> = false | "<location>" | { location, mapping,
/// mapping-mode, same-name-heuristic } }`, with inline mapping values being a
/// pypi name, a list of names, or `false` (normalized to an empty list).
mod conda_pypi_map_serde {
    use serde::{
        Deserialize, Deserializer, Serialize, Serializer,
        de::{Error as _, Unexpected},
        ser::SerializeMap,
    };

    use super::{
        CondaPypiMap, CondaPypiMapEntry, CondaPypiMapSpec, CondaPypiMappingMode, HashMap,
        NamedChannelOrUrl,
    };

    /// Untagged helper for the top level: `false` or a per-channel table.
    #[derive(Deserialize)]
    #[serde(untagged, expecting = "a table of per-channel entries, or `false`")]
    enum TomlCondaPypiMap {
        Toggle(bool),
        Map(HashMap<NamedChannelOrUrl, CondaPypiMapEntry>),
    }

    impl<'de> Deserialize<'de> for CondaPypiMap {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            match TomlCondaPypiMap::deserialize(deserializer)? {
                TomlCondaPypiMap::Toggle(false) => Ok(CondaPypiMap::Disabled),
                TomlCondaPypiMap::Toggle(true) => Err(D::Error::invalid_value(
                    Unexpected::Bool(true),
                    &"`false` to disable the mapping, or a table of per-channel entries",
                )),
                TomlCondaPypiMap::Map(map) => Ok(CondaPypiMap::Map(map)),
            }
        }
    }

    impl Serialize for CondaPypiMap {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                CondaPypiMap::Disabled => serializer.serialize_bool(false),
                CondaPypiMap::Map(map) => map.serialize(serializer),
            }
        }
    }

    /// Untagged helper for one channel's entry: `false`, a bare location
    /// string, or a settings table.
    #[derive(Deserialize)]
    #[serde(
        untagged,
        expecting = "a mapping location, a settings table, or `false`"
    )]
    enum TomlCondaPypiMapEntry {
        Toggle(bool),
        Location(String),
        Spec(TomlCondaPypiMapSpec),
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case", deny_unknown_fields)]
    struct TomlCondaPypiMapSpec {
        #[serde(default)]
        location: Option<String>,
        #[serde(default)]
        mapping: Option<HashMap<String, TomlPypiNames>>,
        #[serde(default)]
        mapping_mode: Option<CondaPypiMappingMode>,
        #[serde(default)]
        same_name_heuristic: Option<bool>,
    }

    /// An inline mapping value: a pypi name, a list of names, or `false`
    /// (meaning "not a PyPI package", normalized to an empty list).
    #[derive(Deserialize)]
    #[serde(untagged, expecting = "a pypi name, a list of pypi names, or `false`")]
    enum TomlPypiNames {
        Toggle(bool),
        One(String),
        Many(Vec<String>),
    }

    impl TomlPypiNames {
        fn into_names<E: serde::de::Error>(self) -> Result<Vec<String>, E> {
            match self {
                TomlPypiNames::Toggle(false) => Ok(Vec::new()),
                TomlPypiNames::Toggle(true) => Err(E::invalid_value(
                    Unexpected::Bool(true),
                    &"a pypi name, a list of pypi names, or `false`",
                )),
                TomlPypiNames::One(name) => Ok(vec![name]),
                TomlPypiNames::Many(names) => Ok(names),
            }
        }
    }

    impl<'de> Deserialize<'de> for CondaPypiMapEntry {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            match TomlCondaPypiMapEntry::deserialize(deserializer)? {
                TomlCondaPypiMapEntry::Toggle(false) => Ok(CondaPypiMapEntry::Disabled),
                TomlCondaPypiMapEntry::Toggle(true) => Err(D::Error::invalid_value(
                    Unexpected::Bool(true),
                    &"`false` to disable the mapping for this channel, or a string or table to configure it",
                )),
                TomlCondaPypiMapEntry::Location(location) => {
                    Ok(CondaPypiMapEntry::from_location(location))
                }
                TomlCondaPypiMapEntry::Spec(spec) => {
                    if spec.location.is_none()
                        && spec.mapping.is_none()
                        && spec.mapping_mode.is_none()
                        && spec.same_name_heuristic.is_none()
                    {
                        return Err(D::Error::custom(
                            "expected at least one of `location`, `mapping`, `mapping-mode` or \
                             `same-name-heuristic`; use `<channel> = false` to disable the mapping",
                        ));
                    }
                    let mapping = spec
                        .mapping
                        .map(|map| {
                            map.into_iter()
                                .map(|(name, names)| Ok((name, names.into_names()?)))
                                .collect::<Result<HashMap<_, _>, D::Error>>()
                        })
                        .transpose()?;
                    Ok(CondaPypiMapEntry::Map(CondaPypiMapSpec {
                        location: spec.location,
                        mapping,
                        mapping_mode: spec.mapping_mode.unwrap_or_default(),
                        same_name_heuristic: spec.same_name_heuristic,
                    }))
                }
            }
        }
    }

    impl Serialize for CondaPypiMapEntry {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                CondaPypiMapEntry::Disabled => serializer.serialize_bool(false),
                // A bare-location spec round-trips to the bare string form.
                CondaPypiMapEntry::Map(CondaPypiMapSpec {
                    location: Some(location),
                    mapping: None,
                    mapping_mode: CondaPypiMappingMode::Overlay,
                    same_name_heuristic: None,
                }) => serializer.serialize_str(location),
                CondaPypiMapEntry::Map(spec) => {
                    let mut map = serializer.serialize_map(None)?;
                    if let Some(location) = &spec.location {
                        map.serialize_entry("location", location)?;
                    }
                    if let Some(mapping) = &spec.mapping {
                        map.serialize_entry("mapping", mapping)?;
                    }
                    map.serialize_entry("mapping-mode", &spec.mapping_mode)?;
                    if let Some(same_name) = spec.same_name_heuristic {
                        map.serialize_entry("same-name-heuristic", &same_name)?;
                    }
                    map.end()
                }
            }
        }
    }
}

#[cfg(test)]
mod conda_pypi_map_serde_tests {
    use super::*;

    fn parse(toml: &str) -> Result<CondaPypiMap, toml_edit::de::Error> {
        toml_edit::de::from_str::<HashMap<String, CondaPypiMap>>(&format!("key = {toml}"))
            .map(|mut m| m.remove("key").unwrap())
    }

    #[test]
    fn test_serde_false_is_disabled() {
        assert_eq!(parse("false").unwrap(), CondaPypiMap::Disabled);
    }

    #[test]
    fn test_serde_true_is_rejected() {
        assert!(parse("true").is_err());
    }

    #[test]
    fn test_serde_bare_location_is_overlay() {
        let map = parse(r#"{ conda-forge = "https://example.com/m.json" }"#).unwrap();
        let CondaPypiMap::Map(entries) = map else {
            panic!("expected a map");
        };
        let entry = &entries[&NamedChannelOrUrl::Name("conda-forge".into())];
        assert_eq!(
            entry,
            &CondaPypiMapEntry::from_location("https://example.com/m.json".into())
        );
    }

    #[test]
    fn test_serde_full_spec_and_channel_false() {
        let map = parse(
            r#"{ conda-forge = { location = "https://example.com/m.json", mapping-mode = "replace", same-name-heuristic = false }, internal = false }"#,
        )
        .unwrap();
        let CondaPypiMap::Map(entries) = map else {
            panic!("expected a map");
        };
        assert_eq!(
            entries[&NamedChannelOrUrl::Name("internal".into())],
            CondaPypiMapEntry::Disabled
        );
        let CondaPypiMapEntry::Map(spec) = &entries[&NamedChannelOrUrl::Name("conda-forge".into())]
        else {
            panic!("expected a spec");
        };
        assert_eq!(spec.location.as_deref(), Some("https://example.com/m.json"));
        assert_eq!(spec.mapping_mode, CondaPypiMappingMode::Replace);
        assert_eq!(spec.same_name_heuristic, Some(false));
    }

    #[test]
    fn test_serde_inline_mapping_values() {
        let map = parse(
            r#"{ conda-forge = { mapping = { conda-name = "pypi-name", multi-name = ["first-name", "second-name"], not-on-pypi = false } } }"#,
        )
        .unwrap();
        let CondaPypiMap::Map(entries) = map else {
            panic!("expected a map");
        };
        let CondaPypiMapEntry::Map(spec) = &entries[&NamedChannelOrUrl::Name("conda-forge".into())]
        else {
            panic!("expected a spec");
        };
        let mapping = spec.mapping.as_ref().unwrap();
        assert_eq!(mapping["conda-name"], vec!["pypi-name".to_string()]);
        assert_eq!(mapping["multi-name"].len(), 2);
        assert!(mapping["not-on-pypi"].is_empty());
    }

    #[test]
    fn test_serde_empty_entry_table_is_rejected() {
        assert!(parse("{ conda-forge = {} }").is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        for toml in [
            "false",
            r#"{ conda-forge = "https://example.com/m.json" }"#,
            r#"{ conda-forge = { location = "https://example.com/m.json", mapping-mode = "replace", same-name-heuristic = false }, internal = false }"#,
        ] {
            let value = parse(toml).unwrap();
            let json = serde_json::to_string(&value).unwrap();
            let back: CondaPypiMap = serde_json::from_str(&json).unwrap();
            assert_eq!(back, value, "round-trip failed for {toml}");
        }
    }
}

// --- toml_span (manifest dialect) ---

impl<'de> toml_span::Deserialize<'de> for CondaPypiMap {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::Boolean(false) => Ok(CondaPypiMap::Disabled),
            ValueInner::Boolean(true) => Err(custom_error(
                "`conda-pypi-map = true` is not supported; use `false` to disable the \
                 mapping, or a table to configure it",
                value.span,
            )
            .into()),
            inner @ ValueInner::Table(_) => {
                let span = value.span;
                let map = TomlHashMap::<NamedChannelOrUrl, CondaPypiMapEntry>::deserialize(
                    &mut Value::with_span(inner, span),
                )?;
                Ok(CondaPypiMap::Map(map.into_inner()))
            }
            other => Err(expected("a table or `false`", other, value.span).into()),
        }
    }
}

impl<'de> toml_span::Deserialize<'de> for CondaPypiMapEntry {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => Ok(CondaPypiMapEntry::from_location(s.into_owned())),
            ValueInner::Boolean(false) => Ok(CondaPypiMapEntry::Disabled),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use `false` to disable the mapping for this \
                 channel, or a string or table to configure it",
                value.span,
            )
            .into()),
            inner @ ValueInner::Table(_) => {
                let table_span = value.span;
                let mut th = TableHelper::new(&mut Value::with_span(inner, table_span))?;

                let location: Option<String> = th.optional("location");
                let mapping: Option<HashMap<String, Vec<String>>> = th
                    .optional::<TomlHashMap<String, TomlCondaPypiMapValue>>("mapping")
                    .map(|map| {
                        map.into_inner()
                            .into_iter()
                            .map(|(name, value)| (name, value.0))
                            .collect()
                    });
                let mapping_mode = th
                    .optional::<TomlEnum<CondaPypiMappingMode>>("mapping-mode")
                    .map(TomlEnum::into_inner);
                let same_name_heuristic = th.optional::<bool>("same-name-heuristic");

                th.finalize(None)?;

                if location.is_none()
                    && mapping.is_none()
                    && same_name_heuristic.is_none()
                    && mapping_mode.is_none()
                {
                    return Err(custom_error(
                        custom_error_message_with_help(
                            "expected at least one of `location`, `mapping`, `mapping-mode` or `same-name-heuristic`",
                            "An empty table has no effect. Use `<channel> = false` to disable the \
                             mapping for this channel, or `<channel> = { mapping-mode = \"replace\" }` \
                             to skip the default mapping data. The same-name heuristic keeps its \
                             per-channel default (on for conda-forge, off otherwise) unless you set \
                             `same-name-heuristic`.",
                        ),
                        table_span,
                    )
                    .into());
                }

                Ok(CondaPypiMapEntry::Map(CondaPypiMapSpec {
                    location,
                    mapping,
                    mapping_mode: mapping_mode.unwrap_or_default(),
                    same_name_heuristic,
                }))
            }
            other => Err(expected("a string, table or `false`", other, value.span).into()),
        }
    }
}

/// The value of an inline mapping entry: a pypi name, a list of pypi names,
/// or `false` to mark the package as not available on PyPI (normalized to an
/// empty list).
pub(crate) struct TomlCondaPypiMapValue(pub(crate) Vec<String>);

impl<'de> toml_span::Deserialize<'de> for TomlCondaPypiMapValue {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => Ok(Self(vec![s.into_owned()])),
            ValueInner::Array(items) => {
                let mut names = Vec::with_capacity(items.len());
                for mut item in items {
                    match item.take() {
                        ValueInner::String(s) => names.push(s.into_owned()),
                        other => return Err(expected("a string", other, item.span).into()),
                    }
                }
                Ok(Self(names))
            }
            ValueInner::Boolean(false) => Ok(Self(Vec::new())),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use a string or a list of strings to map the \
                 package to PyPI name(s), or `false` to mark it as not a PyPI package",
                value.span,
            )
            .into()),
            other => {
                Err(expected("a string, a list of strings or `false`", other, value.span).into())
            }
        }
    }
}
