//! Editing the `exclude-newer` and `pypi-exclude-newer` configuration of a
//! manifest, keeping the parsed configuration and the TOML document in sync.

use std::str::FromStr;

use miette::Diagnostic;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{BinarySpec, ExcludeNewer, PixiSpec};
use pixi_toml_edit::{
    remove_entry, remove_inline_table_entry, remove_table_entry, upsert_entry,
    upsert_inline_table_entry, upsert_table_entry,
};
use rattler_conda_types::{
    ChannelConfig, MatchSpec, PackageName, ParseMatchSpecOptions, RepodataRevision,
};
use toml_edit::{InlineTable, Item, Table, TableLike, Value};

use crate::{
    ExcludeNewerConfig, ExcludeNewerExemption, PypiExcludeNewerConfig, TomlError,
    toml::{InvalidExemptionSpec, validate_exemption_spec},
};

const CONDA_KEY: &str = "exclude-newer";
const PYPI_KEY: &str = "pypi-exclude-newer";

/// An error that occurs while editing the `exclude-newer` configuration.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum ExcludeNewerEditError {
    #[error("'{0}' is not a valid exclude-newer cutoff")]
    #[diagnostic(help(
        "use an RFC 3339 timestamp, a date like `2026-01-01`, or a duration like `7d`"
    ))]
    InvalidCutoff(String),

    #[error("an exclude-newer exemption requires a cutoff")]
    #[diagnostic(help("set a cutoff first, e.g. with `exclude-newer set 7d`"))]
    MissingCutoff,

    #[error("the exclude-newer cutoff cannot be removed while exemptions are configured")]
    #[diagnostic(help("remove the exemptions first with `exclude-newer unexempt`"))]
    ExemptionsRequireCutoff,

    #[error("'{0}' must be a cutoff or a table with a cutoff and exemptions")]
    MalformedEntry(String),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Toml(#[from] TomlError),
}

/// An error that occurs while parsing an exemption from the command line.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum ParseExemptionError {
    #[error("'{spec}' is not a valid match spec")]
    MatchSpec {
        spec: String,
        #[source]
        source: rattler_conda_types::ParseMatchSpecError,
    },

    #[error("the exemption '{0}' must name an exact package")]
    MissingName(String),

    #[error("'{spec}' is not a valid exclude-newer exemption")]
    Invalid {
        spec: String,
        #[source]
        source: InvalidExemptionSpec,
    },

    #[error("the exemption '{0}' cannot have a namespace, it does not select packages")]
    Namespace(String),

    #[error("'{0}' is not a valid PyPI package name")]
    PypiName(String),

    #[error("the PyPI exemption '{0}' can only name a package")]
    #[diagnostic(help(
        "PyPI exemptions exempt every release of a package, version-specific exemptions are not supported yet"
    ))]
    PypiVersion(String),
}

/// Parses a conda exemption the way `pixi add` parses a spec, e.g. `python`,
/// `python=3.13.*` or `conda-forge::numpy[build=h*]`. A bare name exempts
/// every release.
pub fn parse_conda_exemption(
    spec: &str,
    channel_config: &ChannelConfig,
) -> Result<(PackageName, BinarySpec), ParseExemptionError> {
    let match_spec = MatchSpec::from_str(
        spec,
        ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
    )
    .map_err(|source| ParseExemptionError::MatchSpec {
        spec: spec.to_string(),
        source,
    })?;
    let (name, nameless) = match_spec.into_nameless();
    if nameless.namespace.is_some() {
        return Err(ParseExemptionError::Namespace(spec.to_string()));
    }
    let name = name
        .as_exact()
        .cloned()
        .ok_or_else(|| ParseExemptionError::MissingName(spec.to_string()))?;
    let invalid = |source| ParseExemptionError::Invalid {
        spec: spec.to_string(),
        source,
    };
    let binary = PixiSpec::from_nameless_matchspec(nameless, channel_config)
        .into_source_or_binary()
        .right()
        .ok_or(InvalidExemptionSpec::NotBinary)
        .and_then(|spec| validate_exemption_spec(&name, spec))
        .map_err(invalid)?;
    Ok((name, binary))
}

/// Parses a PyPI exemption, which can only be a package name.
pub fn parse_pypi_exemption(spec: &str) -> Result<PypiPackageName, ParseExemptionError> {
    if spec.trim().is_empty() {
        return Err(ParseExemptionError::PypiName(spec.to_string()));
    }
    PypiPackageName::from_str(spec).map_err(|_| {
        if spec.contains(|c: char| "=<>!~*[@; ".contains(c)) {
            ParseExemptionError::PypiVersion(spec.to_string())
        } else {
            ParseExemptionError::PypiName(spec.to_string())
        }
    })
}

/// Converts an `exclude-newer` or `pypi-exclude-newer` entry to a single
/// line value as written, dropping comments and formatting.
pub fn entry_as_written(item: &Item) -> Option<Value> {
    fn normalise(value: &mut Value) {
        value.decor_mut().clear();
        if let Value::InlineTable(table) = value {
            table.set_dotted(false);
            for (_, value) in table.iter_mut() {
                normalise(value);
            }
            table.fmt();
        }
    }
    let mut value = item.clone().into_value().ok()?;
    normalise(&mut value);
    Some(value)
}

impl ExcludeNewerConfig {
    /// Sets or removes the cutoff in both the configuration and `parent`, the
    /// TOML table that holds the `exclude-newer` key.
    pub fn set_cutoff(
        &mut self,
        parent: &mut Table,
        cutoff: Option<&str>,
    ) -> Result<(), ExcludeNewerEditError> {
        let parsed = cutoff.map(parse_cutoff).transpose()?;
        if parsed.is_none() && !self.exemptions.is_empty() {
            return Err(ExcludeNewerEditError::ExemptionsRequireCutoff);
        }
        set_cutoff(parent, CONDA_KEY, cutoff)?;
        self.cutoff = parsed;
        Ok(())
    }

    /// Replaces the configuration with `config`, and the `exclude-newer`
    /// entry of `parent` with `value`, the TOML that `config` was parsed from.
    /// The cutoffs and exemptions are written as in `value`, e.g. `7d` instead
    /// of the duration it parses into.
    pub fn replace(
        &mut self,
        parent: &mut Table,
        value: &str,
        config: ExcludeNewerConfig,
    ) -> Result<(), ExcludeNewerEditError> {
        let mut value = Value::from_str(value)
            .map_err(|_| ExcludeNewerEditError::MalformedEntry(CONDA_KEY.to_string()))?;
        value.decor_mut().clear();
        upsert_table_entry(parent, CONDA_KEY, value);
        *self = config;
        Ok(())
    }

    /// Adds an exemption, or replaces the exemption of the same package, in
    /// both the configuration and `parent`.
    pub fn add_exemption(
        &mut self,
        parent: &mut Table,
        name: PackageName,
        exemption: ExcludeNewerExemption,
    ) -> Result<(), ExcludeNewerEditError> {
        if self.cutoff.is_none() {
            return Err(ExcludeNewerEditError::MissingCutoff);
        }
        // Replacing an exemption with a later cutoff exempts releases that may
        // not have been vetted.
        if let Some(old) = self.exemptions.get(&name).and_then(|old| old.cutoff)
            && exemption.cutoff.is_none_or(|new| exempts_later(new, old))
        {
            let new = exemption
                .cutoff
                .map_or_else(|| "no cutoff".to_string(), crate::format_exclude_newer);
            tracing::warn!(
                "moved the exclude-newer exemption cutoff of {} from {} to {new}",
                name.as_source(),
                crate::format_exclude_newer(old)
            );
        }
        let value = exemption.to_toml_value();
        upsert_exemption(
            parent,
            CONDA_KEY,
            name.as_source(),
            |key| conda_key_matches(key, &name),
            value,
        )?;
        self.exemptions.insert(name, exemption);
        Ok(())
    }

    /// Removes the exemption of `name` from both the configuration and
    /// `parent`. Returns false if the package was not exempt.
    pub fn remove_exemption(
        &mut self,
        parent: &mut Table,
        name: &PackageName,
    ) -> Result<bool, ExcludeNewerEditError> {
        if !self.exemptions.contains_key(name) {
            return Ok(false);
        }
        remove_exemption(parent, CONDA_KEY, |key| conda_key_matches(key, name))?;
        self.exemptions.shift_remove(name);
        Ok(true)
    }
}

impl PypiExcludeNewerConfig {
    /// Sets or removes the cutoff in both the configuration and `parent`, the
    /// TOML table that holds the `pypi-exclude-newer` key. Without a cutoff,
    /// the exemptions fall back to the cutoff of `exclude-newer`.
    pub fn set_cutoff(
        &mut self,
        parent: &mut Table,
        cutoff: Option<&str>,
    ) -> Result<(), ExcludeNewerEditError> {
        let parsed = cutoff.map(parse_cutoff).transpose()?;
        set_cutoff(parent, PYPI_KEY, cutoff)?;
        self.cutoff = parsed;
        Ok(())
    }

    /// Exempts every release of `name` in both the configuration and
    /// `parent`.
    pub fn add_exemption(
        &mut self,
        parent: &mut Table,
        name: PypiPackageName,
    ) -> Result<(), ExcludeNewerEditError> {
        upsert_exemption(
            parent,
            PYPI_KEY,
            name.as_source(),
            |key| pypi_key_matches(key, &name),
            Value::from("*"),
        )?;
        self.exemptions.insert(name, VersionOrStar::Star);
        Ok(())
    }

    /// Removes the exemption of `name` from both the configuration and
    /// `parent`. Returns false if the package was not exempt.
    pub fn remove_exemption(
        &mut self,
        parent: &mut Table,
        name: &PypiPackageName,
    ) -> Result<bool, ExcludeNewerEditError> {
        if !self.exemptions.contains_key(name) {
            return Ok(false);
        }
        remove_exemption(parent, PYPI_KEY, |key| pypi_key_matches(key, name))?;
        self.exemptions.shift_remove(name);
        Ok(true)
    }
}

fn parse_cutoff(cutoff: &str) -> Result<ExcludeNewer, ExcludeNewerEditError> {
    ExcludeNewer::from_str(cutoff).map_err(|_| ExcludeNewerEditError::InvalidCutoff(cutoff.into()))
}

fn conda_key_matches(key: &str, name: &PackageName) -> bool {
    PackageName::from_str(key).is_ok_and(|key| &key == name)
}

fn pypi_key_matches(key: &str, name: &PypiPackageName) -> bool {
    PypiPackageName::from_str(key).is_ok_and(|key| &key == name)
}

/// How the `exclude-newer` key is written in the TOML document.
enum Shape {
    Missing,
    Cutoff,
    Table,
}

fn shape(parent: &Table, key: &str) -> Result<Shape, ExcludeNewerEditError> {
    match parent.get(key) {
        None | Some(Item::None) => Ok(Shape::Missing),
        Some(Item::Value(Value::String(_))) => Ok(Shape::Cutoff),
        Some(Item::Table(_)) | Some(Item::Value(Value::InlineTable(_))) => Ok(Shape::Table),
        Some(_) => Err(ExcludeNewerEditError::MalformedEntry(key.to_string())),
    }
}

/// Sets or removes the cutoff of the `key` entry of `parent`. A cutoff without
/// exemptions is written as a bare string.
fn set_cutoff(
    parent: &mut Table,
    key: &str,
    cutoff: Option<&str>,
) -> Result<(), ExcludeNewerEditError> {
    match (shape(parent, key)?, cutoff) {
        (Shape::Missing, None) => {}
        (Shape::Missing | Shape::Cutoff, Some(cutoff)) => {
            upsert_table_entry(parent, key, cutoff.into());
        }
        (Shape::Cutoff, None) => {
            remove_table_entry(parent, key);
        }
        (Shape::Table, cutoff) => {
            let item = parent.get_mut(key).expect("checked by `shape`");
            match cutoff {
                Some(cutoff) => upsert_entry(item, "cutoff", cutoff.into()),
                None => remove_entry(item, "cutoff").map(|_| ()),
            }
            .map_err(|_| ExcludeNewerEditError::MalformedEntry(key.to_string()))?;
            collapse(parent, key);
        }
    }
    Ok(())
}

/// Inserts `value` as the exemption of `name` into the `key` entry of
/// `parent`, overwriting an existing exemption whose key `matches`. A bare
/// cutoff string is turned into an inline table to make room for it.
fn upsert_exemption(
    parent: &mut Table,
    key: &str,
    name: &str,
    matches: impl Fn(&str) -> bool,
    value: Value,
) -> Result<(), ExcludeNewerEditError> {
    let exemptions_error =
        || ExcludeNewerEditError::from(TomlError::table_error("exemptions", key));
    match shape(parent, key)? {
        Shape::Missing | Shape::Cutoff => {
            let mut table = InlineTable::new();
            if let Some(mut cutoff) = parent.get(key).and_then(Item::as_value).cloned() {
                cutoff.decor_mut().clear();
                table.insert("cutoff", cutoff);
            }
            table.insert("exemptions", InlineTable::from_iter([(name, value)]).into());
            upsert_table_entry(parent, key, table.into());
        }
        Shape::Table => match parent.get_mut(key).expect("checked by `shape`") {
            Item::Table(table) => match (table.is_dotted(), table.get_mut("exemptions")) {
                (_, Some(exemptions)) => {
                    let existing = find_key(
                        exemptions.as_table_like().ok_or_else(exemptions_error)?,
                        &matches,
                    );
                    upsert_entry(exemptions, existing.as_deref().unwrap_or(name), value)
                        .map_err(|_| exemptions_error())?;
                }
                // Dotted keys can't hold a section, so they get an inline
                // table like the other inline layouts.
                (true, None) => upsert_table_entry(
                    table,
                    "exemptions",
                    InlineTable::from_iter([(name, value)]).into(),
                ),
                // A `[exclude-newer]` section gets an `[exclude-newer.exemptions]`
                // section, the layout used in the documentation.
                (false, None) => {
                    let exemptions = Table::from_iter([(name, Item::Value(value))]);
                    table.insert("exemptions", Item::Table(exemptions));
                }
            },
            Item::Value(Value::InlineTable(table)) => match table.get_mut("exemptions") {
                Some(Value::InlineTable(exemptions)) => {
                    let existing = find_key(exemptions, &matches);
                    upsert_inline_table_entry(
                        exemptions,
                        existing.as_deref().unwrap_or(name),
                        value,
                    );
                }
                Some(_) => return Err(exemptions_error()),
                None => upsert_inline_table_entry(
                    table,
                    "exemptions",
                    InlineTable::from_iter([(name, value)]).into(),
                ),
            },
            _ => unreachable!("checked by `shape`"),
        },
    }
    Ok(())
}

/// Removes the exemption whose key `matches` from the `key` entry of `parent`.
fn remove_exemption(
    parent: &mut Table,
    key: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<(), TomlError> {
    let exemptions_error = || TomlError::table_error("exemptions", key);
    match parent.get_mut(key) {
        Some(Item::Table(table)) => {
            if let Some(exemptions) = table.get_mut("exemptions") {
                let existing = find_key(
                    exemptions.as_table_like().ok_or_else(exemptions_error)?,
                    &matches,
                );
                if let Some(existing) = existing {
                    remove_entry(exemptions, &existing).map_err(|_| exemptions_error())?;
                }
            }
        }
        Some(Item::Value(Value::InlineTable(table))) => {
            if let Some(Value::InlineTable(exemptions)) = table.get_mut("exemptions")
                && let Some(existing) = find_key(exemptions, &matches)
            {
                remove_inline_table_entry(exemptions, &existing);
            }
        }
        _ => return Ok(()),
    }
    collapse(parent, key);
    Ok(())
}

/// Returns the key of `table` that `matches`.
fn find_key(table: &dyn TableLike, matches: impl Fn(&str) -> bool) -> Option<String> {
    table
        .iter()
        .map(|(key, _)| key)
        .find(|key| matches(key))
        .map(str::to_string)
}

/// Tidies up the `key` table of `parent` after an edit: drops empty
/// `exemptions`, collapses a lone cutoff back into a bare string and removes
/// the key entirely when nothing is left.
fn collapse(parent: &mut Table, key: &str) {
    let Some(item) = parent.get_mut(key) else {
        return;
    };
    let exemptions_empty = item
        .as_table_like()
        .and_then(|table| table.get("exemptions"))
        .and_then(Item::as_table_like)
        .is_some_and(|exemptions| exemptions.is_empty());
    if exemptions_empty {
        let _ = remove_entry(item, "exemptions");
    }

    let Some(table) = item.as_table_like() else {
        return;
    };
    if table.is_empty() {
        remove_table_entry(parent, key);
    } else if table.len() == 1
        && let Some(cutoff) = table.get("cutoff").and_then(Item::as_value)
        && cutoff.is_str()
    {
        let mut cutoff = cutoff.clone();
        cutoff.decor_mut().clear();
        // Overwrite the table in place, so the key keeps its position and
        // comments.
        upsert_table_entry(parent, key, cutoff);
    }
}

/// Returns whether the exemption cutoff `new` lets through releases that
/// `old` doesn't, now or later.
fn exempts_later(new: ExcludeNewer, old: ExcludeNewer) -> bool {
    match (new, old) {
        (ExcludeNewer::Timestamp(new), ExcludeNewer::Timestamp(old)) => new > old,
        (ExcludeNewer::Duration(new), ExcludeNewer::Duration(old)) => new < old,
        // A duration moves forward in time, past every point in time.
        (ExcludeNewer::Duration(_), ExcludeNewer::Timestamp(_)) => true,
        (ExcludeNewer::Timestamp(new), old) => new > old.cutoff(),
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use toml_edit::DocumentMut;

    use super::*;

    fn conda(source: &str) -> (ExcludeNewerConfig, DocumentMut) {
        let document: DocumentMut = source.parse().unwrap();
        let mut root = toml_span::parse(source).unwrap();
        let config = root
            .pointer_mut("/exclude-newer")
            .map(|value| {
                <crate::toml::TomlExcludeNewer as toml_span::Deserialize>::deserialize(value)
                    .unwrap()
                    .into_inner()
            })
            .unwrap_or_default();
        (config, document)
    }

    fn exempt(source: &str, spec: &str) -> String {
        let (mut config, mut document) = conda(source);
        let (name, spec) =
            parse_conda_exemption(spec, &ChannelConfig::default_with_root_dir("/".into())).unwrap();
        config
            .add_exemption(document.as_table_mut(), name, spec.into())
            .unwrap();
        document.to_string()
    }

    fn unexempt(source: &str, name: &str) -> String {
        let (mut config, mut document) = conda(source);
        assert!(
            config
                .remove_exemption(document.as_table_mut(), &name.parse().unwrap())
                .unwrap()
        );
        document.to_string()
    }

    #[test]
    fn replace_writes_the_value_as_given() {
        let value = r#"{ cutoff = "7d", exemptions = { ruff = { version = ">=0.1", cutoff = "2026-10-12T05:00:00+02:00" } } }"#;
        let (expected, _) = conda(&format!("exclude-newer = {value}\n"));
        let (mut config, mut document) = conda("name = \"x\"\n");
        config
            .replace(document.as_table_mut(), value, expected.clone())
            .unwrap();
        assert_eq!(config, expected);
        assert_snapshot!(document.to_string(), @r#"
        name = "x"
        exclude-newer = { cutoff = "7d", exemptions = { ruff = { version = ">=0.1", cutoff = "2026-10-12T05:00:00+02:00" } } }
        "#);
    }

    #[test]
    fn exempt_turns_cutoff_into_inline_table() {
        assert_snapshot!(exempt("exclude-newer = \"7d\" # comment\nname = \"x\"\n", "python=3.13.*"), @r#"
        exclude-newer = { cutoff = "7d", exemptions = { python = "3.13.*" } } # comment
        name = "x"
        "#);
    }

    #[test]
    fn exempt_bare_name_and_detailed_spec() {
        assert_snapshot!(exempt("exclude-newer = { cutoff = \"7d\", exemptions = { python = \"*\" } }\n", "conda-forge::ruff[build=h*]"), @r#"exclude-newer = { cutoff = "7d", exemptions = { python = "*", ruff = { build = "h*", channel = "conda-forge" } } }"#);
        assert_snapshot!(exempt("exclude-newer = { cutoff = \"7d\" }\n", "polars"), @r#"exclude-newer = { cutoff = "7d", exemptions = { polars = "*" } }"#);
    }

    #[test]
    fn exempt_with_cutoff() {
        let exempt_until = |source: &str, spec: &str, cutoff: &str| {
            let (mut config, mut document) = conda(source);
            let (name, spec) =
                parse_conda_exemption(spec, &ChannelConfig::default_with_root_dir("/".into()))
                    .unwrap();
            let exemption = ExcludeNewerExemption {
                spec,
                cutoff: Some(cutoff.parse().unwrap()),
            };
            config
                .add_exemption(document.as_table_mut(), name, exemption)
                .unwrap();
            document.to_string()
        };
        assert_snapshot!(exempt_until("exclude-newer = \"7d\"\n", "openssl==3.2.1", "2026-10-27T14:03:11Z"), @r#"exclude-newer = { cutoff = "7d", exemptions = { openssl = { version = "==3.2.1", cutoff = "2026-10-27T14:03:11Z" } } }"#);
        assert_snapshot!(exempt_until("exclude-newer = \"7d\"\n", "conda-forge::ruff[build=h*]", "2026-10-27T14:03:11Z"), @r#"exclude-newer = { cutoff = "7d", exemptions = { ruff = { build = "h*", channel = "conda-forge", cutoff = "2026-10-27T14:03:11Z" } } }"#);
        // A date is written as it was given
        assert_snapshot!(exempt_until("exclude-newer = \"7d\"\n", "openssl==3.2.1", "2026-10-12"), @r#"exclude-newer = { cutoff = "7d", exemptions = { openssl = { version = "==3.2.1", cutoff = "2026-10-12" } } }"#);
    }

    #[test]
    fn exempt_replaces_existing_spelling() {
        assert_snapshot!(exempt("exclude-newer = { cutoff = \"7d\", exemptions = { Python = \"*\" } }\n", "python=3.13"), @r#"exclude-newer = { cutoff = "7d", exemptions = { Python = "3.13.*" } }"#);
    }

    #[test]
    fn exempt_in_standard_tables() {
        assert_snapshot!(exempt(r#"[exclude-newer]
cutoff = "7d"

[exclude-newer.exemptions]
python = "*"
"#, "numpy>=2"), @r#"
        [exclude-newer]
        cutoff = "7d"

        [exclude-newer.exemptions]
        python = "*"
        numpy = ">=2"
        "#);
        assert_snapshot!(exempt("[exclude-newer]\ncutoff = \"7d\"\n\n[other]\n", "numpy"), @r#"
        [exclude-newer]
        cutoff = "7d"

        [exclude-newer.exemptions]
        numpy = "*"

        [other]
        "#);
        assert_snapshot!(exempt("exclude-newer.cutoff = \"7d\"\n", "numpy"), @r#"
        exclude-newer.cutoff = "7d"
        exclude-newer.exemptions = { numpy = "*" }
        "#);
    }

    #[test]
    fn exempt_requires_cutoff() {
        let (mut config, mut document) = conda("");
        let err = config
            .add_exemption(
                document.as_table_mut(),
                "python".parse().unwrap(),
                BinarySpec::any().into(),
            )
            .unwrap_err();
        assert!(matches!(err, ExcludeNewerEditError::MissingCutoff));
    }

    #[test]
    fn unexempt_collapses_to_cutoff() {
        assert_snapshot!(unexempt("exclude-newer = { cutoff = \"7d\", exemptions = { python = \"*\", numpy = \"*\" } }\n", "python"), @r#"exclude-newer = { cutoff = "7d", exemptions = { numpy = "*" } }"#);
        assert_snapshot!(unexempt("exclude-newer = { cutoff = \"7d\", exemptions = { python = \"*\" } } # keep\n", "python"), @r#"exclude-newer = "7d" # keep"#);
        assert_snapshot!(unexempt(r#"name = "x"

[exclude-newer]
cutoff = "7d"

[exclude-newer.exemptions]
Python = "*"
"#, "python"), @r#"
        name = "x"
        exclude-newer = "7d"
        "#);
    }

    #[test]
    fn set_and_unset_cutoff() {
        let (mut config, mut document) = conda("exclude-newer = \"7d\" # comment\n");
        config
            .set_cutoff(document.as_table_mut(), Some("2026-01-01"))
            .unwrap();
        assert_snapshot!(document.to_string(), @r#"exclude-newer = "2026-01-01" # comment"#);
        config.set_cutoff(document.as_table_mut(), None).unwrap();
        assert_snapshot!(document.to_string(), @"");

        let (mut config, mut document) =
            conda("exclude-newer = { cutoff = \"7d\", exemptions = { python = \"*\" } }\n");
        config
            .set_cutoff(document.as_table_mut(), Some("14d"))
            .unwrap();
        assert_snapshot!(document.to_string(), @r#"exclude-newer = { cutoff = "14d", exemptions = { python = "*" } }"#);
        let err = config
            .set_cutoff(document.as_table_mut(), None)
            .unwrap_err();
        assert!(matches!(
            err,
            ExcludeNewerEditError::ExemptionsRequireCutoff
        ));

        let (mut config, mut document) = conda("");
        let err = config
            .set_cutoff(document.as_table_mut(), Some("soon"))
            .unwrap_err();
        assert!(matches!(err, ExcludeNewerEditError::InvalidCutoff(_)));
    }

    #[test]
    fn pypi_exemptions_without_cutoff() {
        let mut config = PypiExcludeNewerConfig::default();
        let mut document = DocumentMut::new();
        config
            .add_exemption(document.as_table_mut(), "Torch".parse().unwrap())
            .unwrap();
        assert_snapshot!(document.to_string(), @r#"pypi-exclude-newer = { exemptions = { Torch = "*" } }"#);
        config
            .set_cutoff(document.as_table_mut(), Some("1d"))
            .unwrap();
        assert_snapshot!(document.to_string(), @r#"pypi-exclude-newer = { exemptions = { Torch = "*" }, cutoff = "1d" }"#);
        config.set_cutoff(document.as_table_mut(), None).unwrap();
        assert!(
            config
                .remove_exemption(document.as_table_mut(), &"torch".parse().unwrap())
                .unwrap()
        );
        assert_snapshot!(document.to_string(), @"");
    }

    #[test]
    fn parse_exemptions() {
        let channel_config = ChannelConfig::default_with_root_dir("/".into());
        assert!(parse_conda_exemption("python[extras=[foo]]", &channel_config).is_err());
        // A package archive can be exempted, like in `rattler_config`.
        let (name, spec) =
            parse_conda_exemption("https://example.com/foo-1-0.conda", &channel_config).unwrap();
        assert_eq!(name.as_source(), "foo");
        assert!(matches!(spec, BinarySpec::Url(_)), "{spec:?}");
        assert!(parse_pypi_exemption("torch").is_ok());
        assert!(matches!(
            parse_pypi_exemption("torch==2"),
            Err(ParseExemptionError::PypiVersion(_))
        ));
        assert!(matches!(
            parse_conda_exemption("python[namespace=foo]", &channel_config),
            Err(ParseExemptionError::Namespace(_))
        ));
        assert!(matches!(
            parse_pypi_exemption(""),
            Err(ParseExemptionError::PypiName(_))
        ));
    }
}
