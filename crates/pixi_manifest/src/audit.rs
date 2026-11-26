//! The `[workspace.audit]` policy of `pixi audit`.
//!
//! ```toml
//! [workspace.audit]
//! fail-on = "high"
//! ignore = [
//!   "CVE-2023-0464",
//!   { id = "CVE-2026-1234", reason = "Not reachable", expired-at = "2026-12-01" },
//!   { package = "vendored-thing", reason = "Only used for its data files" },
//! ]
//! ignore-until-fixed = ["CVE-2026-7777"]
//! ```

use std::{fmt, str::FromStr};

use chrono::NaiveDate;
use toml_span::{
    DeserError, Deserialize, ErrorKind, Span, Value,
    de_helpers::{TableHelper, expected},
    value::ValueInner,
};

/// The minimum severity rating that fails an audit. `unknown` is not a
/// threshold: unscored advisories always fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl AuditSeverity {
    pub const ALL: [AuditSeverity; 4] = [
        AuditSeverity::Low,
        AuditSeverity::Medium,
        AuditSeverity::High,
        AuditSeverity::Critical,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AuditSeverity::Low => "low",
            AuditSeverity::Medium => "medium",
            AuditSeverity::High => "high",
            AuditSeverity::Critical => "critical",
        }
    }
}

impl fmt::Display for AuditSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AuditSeverity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|rating| rating.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "'{s}' is not a severity threshold (expected low, medium, high, or critical)"
                )
            })
    }
}

/// An ecosystem an ignore rule can be scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEcosystem {
    CondaForge,
    Pypi,
}

impl AuditEcosystem {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditEcosystem::CondaForge => "conda-forge",
            AuditEcosystem::Pypi => "PyPI",
        }
    }
}

impl fmt::Display for AuditEcosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AuditEcosystem {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("conda-forge") {
            Ok(AuditEcosystem::CondaForge)
        } else if s.eq_ignore_ascii_case("pypi") {
            Ok(AuditEcosystem::Pypi)
        } else {
            Err(format!(
                "unsupported ecosystem '{s}' (expected conda-forge or PyPI)"
            ))
        }
    }
}

/// The two ignore lists of `[workspace.audit]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditIgnoreList {
    /// Rules that apply unconditionally.
    Ignore,
    /// Per-advisory rules that apply while no fixed release newer than the
    /// locked version is reported in
    /// the package's own ecosystem.
    IgnoreUntilFixed,
}

impl AuditIgnoreList {
    pub fn key(self) -> &'static str {
        match self {
            AuditIgnoreList::Ignore => "ignore",
            AuditIgnoreList::IgnoreUntilFixed => "ignore-until-fixed",
        }
    }
}

/// One ignore rule: an advisory id or alias, optionally scoped to a
/// package, ecosystem, and locked version, or every advisory of a package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditIgnoreRule {
    pub id: Option<String>,
    pub package: Option<String>,
    pub ecosystem: Option<AuditEcosystem>,
    /// Matches only this locked version.
    pub version: Option<String>,
    /// Matches locked versions up to and including this one.
    pub up_to_version: Option<String>,
    pub reason: Option<String>,
    /// The last day, in UTC, on which the rule applies.
    pub expired_at: Option<NaiveDate>,
}

/// Why an ignore rule is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAuditIgnoreRule {
    #[error("an ignore rule needs an `id` or a `package`")]
    MissingTarget,
    #[error("an `ignore-until-fixed` rule needs an advisory `id`")]
    UntilFixedWithoutId,
    #[error("'{0}' is not a valid version for the selected ecosystem")]
    InvalidVersion(String),
}

impl AuditIgnoreRule {
    /// A rule that ignores one advisory id or alias everywhere.
    pub fn from_id(id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            ..Self::default()
        }
    }

    /// Checks the rule against the list it is in.
    pub fn validate(&self, list: AuditIgnoreList) -> Result<(), InvalidAuditIgnoreRule> {
        match (&self.id, &self.package, list) {
            (None, None, _) => return Err(InvalidAuditIgnoreRule::MissingTarget),
            (None, Some(_), AuditIgnoreList::IgnoreUntilFixed) => {
                return Err(InvalidAuditIgnoreRule::UntilFixedWithoutId);
            }
            _ => {}
        }
        for version in [&self.version, &self.up_to_version].into_iter().flatten() {
            let conda = || rattler_conda_types::Version::from_str(version).is_ok();
            let pypi = || pep440_rs::Version::from_str(version).is_ok();
            let valid = match self.ecosystem {
                Some(AuditEcosystem::CondaForge) => conda(),
                Some(AuditEcosystem::Pypi) => pypi(),
                None => conda() || pypi(),
            };
            if !valid {
                return Err(InvalidAuditIgnoreRule::InvalidVersion(version.clone()));
            }
        }
        Ok(())
    }

    /// Whether `other` ignores the same thing, comparing identifiers
    /// case-insensitively and versions exactly. Reason and expiry are not
    /// part of what a rule ignores.
    pub fn same_target(&self, other: &Self) -> bool {
        let same = |a: &Option<String>, b: &Option<String>| match (a, b) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            (None, None) => true,
            _ => false,
        };
        // PyPI spells one name several ways; `-`, `_`, and `.` are equal.
        let package = |name: &Option<String>| {
            name.as_ref()
                .map(|name| name.to_ascii_lowercase().replace(['_', '.'], "-"))
        };
        same(&self.id, &other.id)
            && same(&package(&self.package), &package(&other.package))
            && self.ecosystem == other.ecosystem
            && self.version == other.version
            && self.up_to_version == other.up_to_version
    }

    /// The rule as written in a manifest: a bare id when there is nothing
    /// else to record, otherwise an inline table.
    pub fn to_toml_value(&self) -> toml_edit::Value {
        if let Some(id) = &self.id
            && self.package.is_none()
            && self.ecosystem.is_none()
            && self.version.is_none()
            && self.up_to_version.is_none()
            && self.reason.is_none()
            && self.expired_at.is_none()
        {
            return toml_edit::Value::from(id.as_str());
        }
        let mut table = toml_edit::InlineTable::new();
        let entries = [
            ("id", self.id.clone()),
            ("package", self.package.clone()),
            ("ecosystem", self.ecosystem.map(|e| e.as_str().to_string())),
            ("version", self.version.clone()),
            ("up-to-version", self.up_to_version.clone()),
            ("reason", self.reason.clone()),
            (
                "expired-at",
                self.expired_at
                    .map(|date| date.format("%Y-%m-%d").to_string()),
            ),
        ];
        for (key, value) in entries {
            if let Some(value) = value {
                table.insert(key, value.into());
            }
        }
        toml_edit::Value::InlineTable(table)
    }
}

/// The rule as a human reads it, e.g. `CVE-1 in package 'openssl'`.
impl fmt::Display for AuditIgnoreRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ecosystem = self.ecosystem.map(|e| e.as_str().to_string());
        let parts = [
            (None, self.id.as_ref()),
            (Some("package"), self.package.as_ref()),
            (Some("ecosystem"), ecosystem.as_ref()),
            (Some("version"), self.version.as_ref()),
            (Some("up to version"), self.up_to_version.as_ref()),
        ];
        let mut separator = "";
        for (label, value) in parts {
            let Some(value) = value else { continue };
            f.write_str(separator)?;
            match label {
                Some(label) => write!(f, "{label} '{value}'")?,
                None => f.write_str(value)?,
            }
            separator = if label.is_none() { " in " } else { ", " };
        }
        Ok(())
    }
}

/// The `[workspace.audit]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditPolicy {
    pub fail_on: Option<AuditSeverity>,
    pub ignore: Vec<AuditIgnoreRule>,
    pub ignore_until_fixed: Vec<AuditIgnoreRule>,
}

impl AuditPolicy {
    pub fn rules(&self, list: AuditIgnoreList) -> &[AuditIgnoreRule] {
        match list {
            AuditIgnoreList::Ignore => &self.ignore,
            AuditIgnoreList::IgnoreUntilFixed => &self.ignore_until_fixed,
        }
    }

    pub fn rules_mut(&mut self, list: AuditIgnoreList) -> &mut Vec<AuditIgnoreRule> {
        match list {
            AuditIgnoreList::Ignore => &mut self.ignore,
            AuditIgnoreList::IgnoreUntilFixed => &mut self.ignore_until_fixed,
        }
    }
}

fn custom_error(message: impl Into<String>, span: Span) -> toml_span::Error {
    toml_span::Error {
        kind: ErrorKind::Custom(message.into().into()),
        span,
        line_info: None,
    }
}

fn parse_string<T: FromStr<Err = String>>(
    th: &mut TableHelper<'_>,
    key: &'static str,
) -> Option<T> {
    let (_, mut value) = th.take(key)?;
    let span = value.span;
    match value.take() {
        ValueInner::String(text) => match T::from_str(&text) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                th.errors.push(custom_error(err, span));
                None
            }
        },
        inner => {
            th.errors.push(expected("a string", inner, span));
            None
        }
    }
}

/// An `expired-at` date. TOML dates are not supported by the manifest
/// parser, so the date is a `YYYY-MM-DD` string.
struct ExpiryDate(NaiveDate);

impl FromStr for ExpiryDate {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .map(ExpiryDate)
            .map_err(|err| format!("'{s}' is not a date (expected YYYY-MM-DD): {err}"))
    }
}

struct TomlIgnoreRule(AuditIgnoreRule, Span);

impl<'de> Deserialize<'de> for TomlIgnoreRule {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let span = value.span;
        match value.take() {
            ValueInner::String(id) => Ok(Self(AuditIgnoreRule::from_id(id.into_owned()), span)),
            inner @ ValueInner::Table(_) => {
                let mut table = Value::with_span(inner, span);
                let mut th = TableHelper::new(&mut table)?;
                let rule = AuditIgnoreRule {
                    id: th.optional("id"),
                    package: th.optional("package"),
                    ecosystem: parse_string(&mut th, "ecosystem"),
                    version: th.optional("version"),
                    up_to_version: th.optional("up-to-version"),
                    reason: th.optional("reason"),
                    expired_at: parse_string::<ExpiryDate>(&mut th, "expired-at").map(|d| d.0),
                };
                th.finalize(None)?;
                Ok(Self(rule, span))
            }
            inner => Err(expected("an advisory id or a table", inner, span).into()),
        }
    }
}

fn parse_rules(th: &mut TableHelper<'_>, list: AuditIgnoreList) -> Vec<AuditIgnoreRule> {
    let Some(entries) = th.optional::<Vec<TomlIgnoreRule>>(list.key()) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|TomlIgnoreRule(rule, span)| match rule.validate(list) {
            Ok(()) => Some(rule),
            Err(err) => {
                th.errors.push(custom_error(err.to_string(), span));
                None
            }
        })
        .collect()
}

impl<'de> Deserialize<'de> for AuditPolicy {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        let mut th = TableHelper::new(value)?;
        let fail_on = parse_string(&mut th, "fail-on");
        let ignore = parse_rules(&mut th, AuditIgnoreList::Ignore);
        let ignore_until_fixed = parse_rules(&mut th, AuditIgnoreList::IgnoreUntilFixed);
        th.finalize(None)?;
        Ok(Self {
            fail_on,
            ignore,
            ignore_until_fixed,
        })
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use pixi_test_utils::format_parse_error;

    use super::*;
    use crate::toml::FromTomlStr;

    fn parse(input: &str) -> Result<AuditPolicy, String> {
        AuditPolicy::from_toml_str(input).map_err(|err| format_parse_error(input, err))
    }

    #[test]
    fn parses_both_lists_and_rule_shapes() {
        let policy = parse(
            r#"
            fail-on = "high"
            ignore = [
              "CVE-2023-0464",
              { id = "CVE-2026-1234", package = "openssl", ecosystem = "conda-forge", up-to-version = "3.1.0", reason = "not reachable", expired-at = "2026-12-01" },
              { package = "vendored" },
            ]
            ignore-until-fixed = [{ id = "CVE-2026-7777", package = "Requests", ecosystem = "pypi" }]
            "#,
        )
        .unwrap();
        assert_eq!(policy.fail_on, Some(AuditSeverity::High));
        assert_eq!(policy.ignore.len(), 3);
        assert_eq!(
            policy.ignore[1].expired_at,
            NaiveDate::from_ymd_opt(2026, 12, 1)
        );
        assert_eq!(
            policy.ignore_until_fixed[0].ecosystem,
            Some(AuditEcosystem::Pypi)
        );
    }

    #[test]
    fn rejects_unknown_and_removed_keys() {
        assert_snapshot!(parse("ignore-unfixed = true").unwrap_err(), @"
         × Unexpected keys, expected only 'fail-on', 'ignore', 'ignore-until-fixed'
          ╭─[pixi.toml:1:1]
        1 │ ignore-unfixed = true
          · ───────┬──────
          ·        ╰── 'ignore-unfixed' was not expected here
          ╰────
         help: Did you mean 'ignore-until-fixed'?
        ");
        assert_snapshot!(parse(r#"ignore = [{ id = "CVE-1", expires = "2026-01-01" }]"#).unwrap_err(), @r#"
         × Unexpected keys, expected only 'id', 'package', 'ecosystem', 'version', 'up-to-version', 'reason', 'expired-at'
          ╭─[pixi.toml:1:27]
        1 │ ignore = [{ id = "CVE-1", expires = "2026-01-01" }]
          ·                           ───┬───
          ·                              ╰── 'expires' was not expected here
          ╰────
         help: Did you mean 'expired-at'?
        "#);
    }

    #[test]
    fn rejects_invalid_rules() {
        assert_snapshot!(parse(r#"ignore-until-fixed = [{ package = "openssl" }]"#).unwrap_err(), @r#"
         × an `ignore-until-fixed` rule needs an advisory `id`
          ╭─[pixi.toml:1:23]
        1 │ ignore-until-fixed = [{ package = "openssl" }]
          ·                       ───────────────────────
          ╰────
        "#);
        assert_snapshot!(parse(r#"ignore = [{ reason = "everything" }]"#).unwrap_err(), @r#"
         × an ignore rule needs an `id` or a `package`
          ╭─[pixi.toml:1:11]
        1 │ ignore = [{ reason = "everything" }]
          ·           ─────────────────────────
          ╰────
        "#);
        assert_snapshot!(parse(r#"fail-on = "unknown""#).unwrap_err(), @r#"
         × 'unknown' is not a severity threshold (expected low, medium, high, or critical)
          ╭─[pixi.toml:1:12]
        1 │ fail-on = "unknown"
          ·            ───────
          ╰────
        "#);
        assert_snapshot!(parse(r#"ignore = [{ id = "CVE-1", ecosystem = "npm" }]"#).unwrap_err(), @r#"
         × unsupported ecosystem 'npm' (expected conda-forge or PyPI)
          ╭─[pixi.toml:1:40]
        1 │ ignore = [{ id = "CVE-1", ecosystem = "npm" }]
          ·                                        ───
          ╰────
        "#);
        assert_snapshot!(parse(r#"ignore = [{ id = "CVE-1", ecosystem = "pypi", version = "not/a/version" }]"#).unwrap_err(), @r#"
         × 'not/a/version' is not a valid version for the selected ecosystem
          ╭─[pixi.toml:1:11]
        1 │ ignore = [{ id = "CVE-1", ecosystem = "pypi", version = "not/a/version" }]
          ·           ───────────────────────────────────────────────────────────────
          ╰────
        "#);
        assert_snapshot!(parse(r#"ignore = [{ id = "CVE-1", expired-at = "December" }]"#).unwrap_err(), @r#"
         × 'December' is not a date (expected YYYY-MM-DD): input contains invalid characters
          ╭─[pixi.toml:1:41]
        1 │ ignore = [{ id = "CVE-1", expired-at = "December" }]
          ·                                         ────────
          ╰────
        "#);
    }

    #[test]
    fn rules_render_compactly() {
        assert_eq!(
            AuditIgnoreRule::from_id("CVE-1")
                .to_toml_value()
                .to_string(),
            r#""CVE-1""#
        );
        let rule = AuditIgnoreRule {
            id: Some("CVE-1".to_string()),
            package: Some("openssl".to_string()),
            ecosystem: Some(AuditEcosystem::CondaForge),
            reason: Some("not reachable".to_string()),
            expired_at: NaiveDate::from_ymd_opt(2026, 12, 1),
            ..AuditIgnoreRule::default()
        };
        assert_eq!(
            rule.to_toml_value().to_string(),
            r#"{ id = "CVE-1", package = "openssl", ecosystem = "conda-forge", reason = "not reachable", expired-at = "2026-12-01" }"#
        );
        assert!(rule.same_target(&AuditIgnoreRule {
            id: Some("cve-1".to_string()),
            package: Some("OpenSSL".to_string()),
            ecosystem: Some(AuditEcosystem::CondaForge),
            ..AuditIgnoreRule::default()
        }));
    }
}
