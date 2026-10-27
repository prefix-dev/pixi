use std::fmt::Write;

use chrono::{DateTime, SecondsFormat, Utc};
use indexmap::IndexMap;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{
    BinarySpec, DetailedSpec, ExcludeNewer, InvalidExemptionError, PixiSpec, ResolvedExcludeNewer,
    SpecConversionError,
};
use rattler_conda_types::{ChannelConfig, MatchSpec, PackageName, ParseChannelError};
use serde::ser::SerializeMap;

use crate::{PrioritizedChannel, pypi::ResolvedPypiExcludeNewer};

/// The `exclude-newer` configuration of a workspace for conda packages.
///
/// In the manifest this is either a bare cutoff:
///
/// ```toml
/// [workspace]
/// exclude-newer = "7d"
/// ```
///
/// or a table with a cutoff and exemptions:
///
/// ```toml
/// [workspace.exclude-newer]
/// cutoff = "7d"
/// exemptions = { py-rattler = "*", polars = "==1.43.1" }
/// ```
///
/// An exemption can also have a `cutoff` of its own, an absolute point in
/// time, after which matching releases are no longer exempt:
///
/// ```toml
/// [workspace.exclude-newer.exemptions]
/// openssl = { version = "==3.2.1", cutoff = "2026-10-12T00:00:00Z" }
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExcludeNewerConfig {
    /// Packages uploaded after this cutoff are excluded from the solve.
    pub cutoff: Option<ExcludeNewer>,

    /// Package releases that are not excluded, regardless of their
    /// timestamp. The spec is a binary spec, so `polars = "==1.43.1"`
    /// exempts that release only and `py-rattler = "*"` exempts every
    /// release.
    pub exemptions: IndexMap<PackageName, ExcludeNewerExemption>,
}

/// An exemption from the `exclude-newer` cutoff of a package.
#[derive(Debug, Clone, PartialEq)]
pub struct ExcludeNewerExemption {
    /// The releases that are exempt.
    pub spec: BinarySpec,

    /// Releases published after this point in time are not exempt, even if
    /// they match [`Self::spec`]. This restricts the exemption to the releases
    /// that existed when they were vetted, e.g. a later build of the same
    /// version is not exempt.
    pub cutoff: Option<DateTime<Utc>>,
}

impl ExcludeNewerExemption {
    /// Returns whether the exemption allows nothing beyond `effective_cutoff`,
    /// the earliest cutoff of the channels it applies to. Only exemptions with
    /// a cutoff can become stale.
    fn is_stale(&self, effective_cutoff: DateTime<Utc>) -> bool {
        self.cutoff.is_some_and(|cutoff| cutoff <= effective_cutoff)
    }
}

impl ExcludeNewerExemption {
    /// Returns the exemption as it is written in the manifest: the spec, with
    /// the cutoff added to its table if there is one.
    pub fn to_toml_value(&self) -> toml_edit::Value {
        let spec = PixiSpec::from(self.spec.clone()).to_toml_value();
        let Some(cutoff) = self.cutoff else {
            return spec;
        };
        let mut table = match spec {
            toml_edit::Value::InlineTable(table) => table,
            version => {
                let mut table = toml_edit::InlineTable::new();
                table.insert("version", version);
                table
            }
        };
        table.insert("cutoff", format_cutoff(cutoff).into());
        table.into()
    }
}

/// Formats an absolute cutoff the way it is written to the manifest.
pub fn format_cutoff(cutoff: DateTime<Utc>) -> String {
    cutoff.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

impl From<BinarySpec> for ExcludeNewerExemption {
    fn from(spec: BinarySpec) -> Self {
        Self { spec, cutoff: None }
    }
}

impl serde::Serialize for ExcludeNewerExemption {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(serde::Serialize)]
        struct WithCutoff<'a> {
            #[serde(flatten)]
            spec: &'a DetailedSpec,
            cutoff: ExcludeNewer,
        }

        let Some(cutoff) = self.cutoff else {
            return self.spec.serialize(serializer);
        };
        let detailed = match &self.spec {
            BinarySpec::Version(version) => DetailedSpec {
                version: Some(version.clone()),
                ..DetailedSpec::default()
            },
            BinarySpec::DetailedVersion(detailed) => (**detailed).clone(),
            BinarySpec::Url(_) | BinarySpec::Path(_) => {
                return Err(serde::ser::Error::custom(
                    "an exclude-newer exemption must be a version or a match spec table",
                ));
            }
        };
        WithCutoff {
            spec: &detailed,
            cutoff: ExcludeNewer::Timestamp(cutoff),
        }
        .serialize(serializer)
    }
}

impl ExcludeNewerConfig {
    /// Creates a configuration that consists of a cutoff only.
    pub fn from_cutoff(cutoff: ExcludeNewer) -> Self {
        Self {
            cutoff: Some(cutoff),
            exemptions: IndexMap::new(),
        }
    }

    /// Returns true if neither a cutoff nor exemptions are configured.
    pub fn is_empty(&self) -> bool {
        self.cutoff.is_none() && self.exemptions.is_empty()
    }
}

impl From<ExcludeNewer> for ExcludeNewerConfig {
    fn from(cutoff: ExcludeNewer) -> Self {
        Self::from_cutoff(cutoff)
    }
}

impl serde::Serialize for ExcludeNewerConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match (&self.cutoff, self.exemptions.is_empty()) {
            (Some(cutoff), true) => cutoff.serialize(serializer),
            (cutoff, _) => {
                let mut map = serializer.serialize_map(None)?;
                if let Some(cutoff) = cutoff {
                    map.serialize_entry("cutoff", cutoff)?;
                }
                if !self.exemptions.is_empty() {
                    map.serialize_entry("exemptions", &self.exemptions)?;
                }
                map.end()
            }
        }
    }
}

/// The `pypi-exclude-newer` configuration of a workspace for PyPI packages.
///
/// Mirrors [`ExcludeNewerConfig`]. Without a cutoff of its own, the conda
/// cutoff of `exclude-newer` applies to PyPI packages as well.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PypiExcludeNewerConfig {
    /// PyPI packages uploaded after this cutoff are excluded from the solve.
    /// Falls back to the cutoff of [`ExcludeNewerConfig`] when unset.
    pub cutoff: Option<ExcludeNewer>,

    /// PyPI packages that are never excluded, regardless of their timestamp.
    /// Only `"*"` is supported for now, which exempts every release of the
    /// package.
    pub exemptions: IndexMap<PypiPackageName, VersionOrStar>,
}

impl PypiExcludeNewerConfig {
    /// Returns true if neither a cutoff nor exemptions are configured.
    pub fn is_empty(&self) -> bool {
        self.cutoff.is_none() && self.exemptions.is_empty()
    }
}

/// An error that occurs while resolving the exclude-newer configuration of a
/// manifest into absolute cutoffs.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum ExcludeNewerError {
    #[error("failed to resolve the channel for the exclude-newer configuration")]
    Channel(#[from] ParseChannelError),

    #[error("the exclude-newer exemption for '{}' is not a valid match spec", .name.as_source())]
    Exemption {
        name: PackageName,
        #[source]
        source: SpecConversionError,
    },

    #[error(transparent)]
    InvalidExemption(#[from] InvalidExemptionError),
}

/// Combines the workspace `exclude-newer` configuration with the channel
/// overrides into absolute cutoffs for the conda solver.
///
/// Without a base cutoff, packages are only excluded when one of the channel
/// overrides or deprecated package overrides applies to them. The exemptions
/// are converted into match specs against `channel_config`.
///
/// `package_overrides` are the per-package cutoffs of the deprecated top-level
/// `[exclude-newer]` table. They keep working until that table is removed.
pub fn resolve_exclude_newer<'a>(
    config: &ExcludeNewerConfig,
    channels: impl IntoIterator<Item = &'a PrioritizedChannel>,
    channel_config: &ChannelConfig,
    package_overrides: impl IntoIterator<Item = (&'a PackageName, &'a ExcludeNewer)>,
) -> Result<Option<ResolvedExcludeNewer>, ExcludeNewerError> {
    let mut exclude_newer = config
        .cutoff
        .map(|cutoff| ResolvedExcludeNewer::from_datetime(cutoff.cutoff()));

    for channel in channels {
        let Some(channel_exclude_newer) = channel.exclude_newer else {
            continue;
        };

        let channel_key = channel.channel.clone().into_base_url(channel_config)?;
        let config = exclude_newer
            .get_or_insert_with(|| ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC));
        *config = config
            .clone()
            .with_channel_cutoff(channel_key, channel_exclude_newer.cutoff());
    }

    #[allow(deprecated)]
    for (name, package_exclude_newer) in package_overrides {
        let config = exclude_newer
            .get_or_insert_with(|| ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC));
        *config = config
            .clone()
            .with_package_cutoff(name.clone(), package_exclude_newer.cutoff());
    }

    // Exemptions only matter when something is excluded in the first place.
    if let Some(mut resolved) = exclude_newer.take() {
        for (name, exemption) in &config.exemptions {
            let spec = exemption_match_spec(name, exemption, channel_config)?;
            resolved = resolved.with_exemption_until(spec, exemption.cutoff)?;
        }
        exclude_newer = Some(resolved);
    }

    Ok(exclude_newer)
}

fn exemption_match_spec(
    name: &PackageName,
    exemption: &ExcludeNewerExemption,
    channel_config: &ChannelConfig,
) -> Result<MatchSpec, ExcludeNewerError> {
    exemption
        .spec
        .clone()
        .to_match_spec(name, channel_config)
        .map_err(|source| ExcludeNewerError::Exemption {
            name: name.clone(),
            source,
        })
}

/// Returns the exemptions of `config` that are stale at `now`: exemptions
/// with a cutoff that do not exempt anything, because the cutoff of every
/// channel they can apply to is already at or past the cutoff of the
/// exemption.
///
/// `channels` are all channels the exemptions can apply to, e.g. the channels
/// of every environment, and `package_overrides` are the per-package cutoffs
/// of the deprecated top-level `[exclude-newer]` table. Relative cutoffs only
/// move forward in time, so a stale exemption stays stale unless the
/// configuration changes.
pub fn stale_exemptions<'a>(
    config: &ExcludeNewerConfig,
    channels: impl IntoIterator<Item = &'a PrioritizedChannel>,
    channel_config: &ChannelConfig,
    package_overrides: impl IntoIterator<Item = (&'a PackageName, &'a ExcludeNewer)>,
    now: DateTime<Utc>,
) -> Result<Vec<PackageName>, ExcludeNewerError> {
    let default_cutoff = config
        .cutoff
        .map_or(DateTime::<Utc>::MAX_UTC, |cutoff| cutoff.cutoff_at(now));
    let channel_cutoffs = channels
        .into_iter()
        .map(|channel| {
            let url = channel.channel.clone().into_base_url(channel_config)?;
            let cutoff = channel
                .exclude_newer
                .map_or(default_cutoff, |cutoff| cutoff.cutoff_at(now));
            Ok((url, cutoff))
        })
        .collect::<Result<Vec<_>, ParseChannelError>>()?;
    let package_overrides = package_overrides
        .into_iter()
        .map(|(name, cutoff)| (name, cutoff.cutoff_at(now)))
        .collect::<std::collections::HashMap<_, _>>();

    let mut stale = Vec::new();
    for (name, exemption) in &config.exemptions {
        if exemption.cutoff.is_none() {
            continue;
        }
        // The deprecated per-package cutoffs take precedence over the
        // channel cutoffs.
        let effective_cutoff = match package_overrides.get(name) {
            Some(cutoff) => *cutoff,
            None => {
                let spec = exemption_match_spec(name, exemption, channel_config)?;
                match spec.channel {
                    // An exemption for a channel that is not used yet applies
                    // once the channel is added, with the default cutoff
                    // unless the channel gets a cutoff of its own.
                    Some(channel) => channel_cutoffs
                        .iter()
                        .filter(|(url, _)| *url == channel.base_url)
                        .map(|(_, cutoff)| *cutoff)
                        .min()
                        .unwrap_or(default_cutoff),
                    None => channel_cutoffs
                        .iter()
                        .map(|(_, cutoff)| *cutoff)
                        .min()
                        .unwrap_or(default_cutoff),
                }
            }
        };
        if exemption.is_stale(effective_cutoff) {
            stale.push(name.clone());
        }
    }
    Ok(stale)
}

/// Combines the workspace `pypi-exclude-newer` configuration with the conda
/// cutoff it falls back to into absolute cutoffs for the PyPI solver.
///
/// `package_overrides` are the per-package cutoffs of the deprecated top-level
/// `[pypi-exclude-newer]` table. They keep working until that table is
/// removed.
pub fn resolve_pypi_exclude_newer<'a>(
    config: &PypiExcludeNewerConfig,
    fallback_cutoff: Option<ExcludeNewer>,
    package_overrides: impl IntoIterator<Item = (&'a PypiPackageName, &'a ExcludeNewer)>,
) -> ResolvedPypiExcludeNewer {
    let mut exclude_newer = config
        .cutoff
        .or(fallback_cutoff)
        .map(|cutoff| ResolvedPypiExcludeNewer::from_datetime(cutoff.cutoff()))
        .unwrap_or_default();

    #[allow(deprecated)]
    for (name, package_exclude_newer) in package_overrides {
        exclude_newer = exclude_newer
            .with_package_cutoff(name.as_normalized().clone(), package_exclude_newer.cutoff());
    }

    for (name, version) in &config.exemptions {
        // uv can only lift the cutoff for a whole package, so the manifest
        // parser rejects version-specific exemptions. Treating one as `*`
        // here would exempt more releases than requested.
        assert!(
            matches!(version, VersionOrStar::Star),
            "the PyPI exclude-newer exemption `{} = \"{version}\"` should have been rejected by the manifest parser",
            name.as_source()
        );
        exclude_newer = exclude_newer.with_exempt_package(name.as_normalized().clone());
    }

    exclude_newer
}

/// The migration of a deprecated top-level `[exclude-newer]` or
/// `[pypi-exclude-newer]` table of per-package cutoffs, for use in the
/// deprecation warning.
#[derive(Debug, Clone)]
pub struct ExcludeNewerTableMigration {
    /// Whether any of the entries can be replaced by an exemption.
    pub has_replacement: bool,

    /// The help text that suggests the replacement, or announces the removal
    /// of the entries without one.
    pub help: String,
}

impl ExcludeNewerTableMigration {
    /// Builds the migration of the deprecated per-package cutoffs of `table`
    /// to the `exemptions` of `[workspace.{table}]`.
    ///
    /// A zero cutoff such as `0d` lifts the cutoff for every release of the
    /// package, which is what the wildcard exemption `"*"` does. Any other
    /// per-package cutoff has no replacement and will stop working once the
    /// table is removed.
    ///
    /// `cutoff` and `exemptions` are the current `[workspace.{table}]`, and
    /// `other_cutoffs` tells whether anything is excluded without `cutoff`:
    /// channel cutoffs for conda, the conda cutoff for PyPI. `prefix` is the
    /// prefix of the pixi tables, such as `tool.pixi`.
    pub fn new<'a>(
        prefix: Option<&str>,
        table: &str,
        cutoff: Option<ExcludeNewer>,
        exemptions: impl IntoIterator<Item = (&'a str, toml_edit::Value)>,
        other_cutoffs: bool,
        overrides: impl IntoIterator<Item = (&'a str, &'a ExcludeNewer)>,
    ) -> Self {
        let (zero_cutoffs, without_replacement): (Vec<_>, Vec<_>) = overrides
            .into_iter()
            .partition(|(_, cutoff)| matches!(cutoff, ExcludeNewer::Duration(d) if d.is_zero()));
        let list = |packages: &[(&str, _)]| {
            packages
                .iter()
                .map(|(package, _)| format!("`{package}`"))
                .join(", ")
        };

        let prefix = prefix
            .map(|prefix| format!("{prefix}."))
            .unwrap_or_default();
        let mut help = String::new();
        let mut has_replacement = false;
        if zero_cutoffs.is_empty() {
            // There is nothing to move to the exemptions.
        } else if cutoff.is_none() && !other_cutoffs {
            // Nothing is excluded apart from the per-package cutoffs, so a
            // zero cutoff lifts nothing.
            let _ = write!(
                help,
                "the zero cutoffs of {} have no effect without a cutoff, remove them",
                list(&zero_cutoffs)
            );
        } else if cutoff.is_none() && table == "exclude-newer" {
            // Conda exemptions require a cutoff in `[workspace.exclude-newer]`,
            // while PyPI exemptions fall back to the conda cutoff.
            let _ = write!(
                help,
                "set a `cutoff` in `[{prefix}workspace.{table}]` to replace the zero cutoffs of {} with `exemptions`, note that it also applies to the channels without their own `exclude-newer`",
                list(&zero_cutoffs)
            );
        } else {
            has_replacement = true;
            let entries = if without_replacement.is_empty() {
                "the entries"
            } else {
                "the entries with a zero cutoff"
            };
            let _ = write!(
                help,
                "move {entries} to the `exemptions` of `[{prefix}workspace.{table}]` instead, e.g. replace the current `{table}` of `[{prefix}workspace]` with\n\n[{prefix}workspace.{table}]"
            );
            match cutoff {
                // The `Display` of a timestamp is not a valid cutoff.
                Some(ExcludeNewer::Timestamp(timestamp)) => {
                    let timestamp = timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true);
                    let _ = write!(help, "\ncutoff = \"{timestamp}\"");
                }
                Some(cutoff @ ExcludeNewer::Duration(_)) => {
                    let _ = write!(help, "\ncutoff = \"{cutoff}\"");
                }
                None => {}
            }
            let exemptions = exemptions
                .into_iter()
                .filter(|(package, _)| !zero_cutoffs.iter().any(|(zero, _)| zero == package))
                .chain(
                    zero_cutoffs
                        .iter()
                        .map(|(package, _)| (*package, toml_edit::Value::from("*"))),
                )
                .map(|(package, spec)| format!("{} = {spec}", toml_edit::Key::new(package)))
                .join(", ");
            let _ = write!(help, "\nexemptions = {{ {exemptions} }}");
            if table == "pypi-exclude-newer" {
                help.push_str(
                    "\n\nAn exemption allows every release of the package regardless of its upload time.",
                );
            } else {
                help.push_str(
                    "\n\nAn exemption allows the matching releases regardless of their upload time. Narrow `\"*\"` down to the releases you vetted, e.g. `\"1.2.3\"`.",
                );
            }
        }
        if !without_replacement.is_empty() {
            if !help.is_empty() {
                help.push_str("\n\n");
            }
            let _ = write!(
                help,
                "the per-package cutoffs of {} have no direct replacement and stop working when the table is removed. To allow newer releases of a package, add the versions you vetted to `exemptions`; to keep a package on older releases, constrain its version in the dependencies.",
                list(&without_replacement)
            );
        }

        Self {
            has_replacement,
            help,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::NamedChannelOrUrl;

    use super::*;

    fn exemption(spec: &str, cutoff: Option<&str>) -> ExcludeNewerExemption {
        let spec = match spec.strip_prefix("conda-forge::") {
            Some(version) => BinarySpec::DetailedVersion(Box::new(DetailedSpec {
                version: Some(version.parse().unwrap()),
                channel: Some(NamedChannelOrUrl::Name("conda-forge".into())),
                ..DetailedSpec::default()
            })),
            None => BinarySpec::Version(spec.parse().unwrap()),
        };
        ExcludeNewerExemption {
            spec,
            cutoff: cutoff.map(|cutoff| cutoff.parse().unwrap()),
        }
    }

    fn channel(name: &str, exclude_newer: Option<&str>) -> PrioritizedChannel {
        PrioritizedChannel {
            channel: NamedChannelOrUrl::Name(name.into()),
            priority: None,
            exclude_newer: exclude_newer.map(|cutoff| ExcludeNewer::from_str(cutoff).unwrap()),
        }
    }

    fn stale(config: &ExcludeNewerConfig, channels: &[PrioritizedChannel]) -> Vec<String> {
        stale_exemptions(
            config,
            channels,
            &ChannelConfig::default_with_root_dir("/".into()),
            [],
            "2026-10-27T00:00:00Z".parse().unwrap(),
        )
        .unwrap()
        .into_iter()
        .map(|name| name.as_source().to_string())
        .collect()
    }

    #[test]
    fn test_stale_exemptions() {
        let config = ExcludeNewerConfig {
            // 2026-10-20T00:00:00Z
            cutoff: Some(ExcludeNewer::from_str("7d").unwrap()),
            exemptions: IndexMap::from_iter([
                (
                    "before".parse().unwrap(),
                    exemption("==1", Some("2026-10-19T00:00:00Z")),
                ),
                (
                    "at".parse().unwrap(),
                    exemption("==1", Some("2026-10-20T00:00:00Z")),
                ),
                (
                    "after".parse().unwrap(),
                    exemption("==1", Some("2026-10-21T00:00:00Z")),
                ),
                ("unbounded".parse().unwrap(), exemption("==1", None)),
            ]),
        };
        assert_eq!(
            stale(&config, &[channel("conda-forge", None)]),
            ["before", "at"]
        );
        // A channel with a later cutoff doesn't make the exemptions stale for
        // the other channels.
        assert_eq!(
            stale(
                &config,
                &[
                    channel("conda-forge", None),
                    channel("internal", Some("0d"))
                ]
            ),
            ["before", "at"]
        );
        // An exemption is only stale once it is stale for every channel.
        assert_eq!(
            stale(
                &config,
                &[
                    channel("conda-forge", None),
                    channel("pytorch", Some("2026-10-18T00:00:00Z"))
                ]
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_stale_exemptions_of_channel() {
        let config = ExcludeNewerConfig {
            cutoff: Some(ExcludeNewer::from_str("7d").unwrap()),
            exemptions: IndexMap::from_iter([(
                "polars".parse().unwrap(),
                exemption("conda-forge::==1", Some("2026-10-19T00:00:00Z")),
            )]),
        };
        // Only the cutoff of the channel of the exemption matters.
        assert_eq!(
            stale(
                &config,
                &[
                    channel("conda-forge", None),
                    channel("pytorch", Some("2026-10-18T00:00:00Z"))
                ]
            ),
            ["polars"]
        );
        assert_eq!(
            stale(
                &config,
                &[channel("conda-forge", Some("2026-10-18T00:00:00Z"))]
            ),
            Vec::<String>::new()
        );
    }
}
