use std::fmt::Write;

use chrono::{DateTime, NaiveTime, SecondsFormat, Utc};
use indexmap::IndexMap;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{
    BinarySpec, ConfiguredCutoffs, DetailedSpec, ExcludeNewer, InvalidExemptionError, PixiSpec,
    ResolvedExcludeNewer, SpecConversionError,
};
use rattler_conda_types::{ChannelConfig, MatchSpec, PackageName, ParseChannelError};
use serde::ser::SerializeMap;

use crate::{
    PrioritizedChannel,
    pypi::ResolvedPypiExcludeNewer,
    toml::{InvalidExemptionSpec, validate_exemption_spec},
};

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
/// An exemption can also have a `cutoff` of its own, after which matching
/// releases are no longer exempt:
///
/// ```toml
/// [workspace.exclude-newer.exemptions]
/// openssl = { version = "==3.2.1", cutoff = "2026-10-12T00:00:00Z" }
/// ruff = { version = "*", cutoff = "1d" }
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

    /// Releases published after this cutoff are not exempt, even if they
    /// match [`Self::spec`]. A point in time restricts the exemption to the
    /// releases that existed when they were vetted, e.g. a later build of the
    /// same version is not exempt.
    pub cutoff: Option<ExcludeNewer>,
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
        table.insert("cutoff", format_exclude_newer(cutoff).into());
        table.into()
    }
}

/// Formats a cutoff the way it is written to the manifest, see
/// [`format_cutoff`]. A duration is written in its largest whole unit, e.g.
/// `7d`.
pub fn format_exclude_newer(cutoff: ExcludeNewer) -> String {
    let duration = match cutoff {
        ExcludeNewer::Timestamp(cutoff) => return format_cutoff(cutoff),
        ExcludeNewer::Duration(duration) => duration,
    };
    if duration.subsec_nanos() != 0 {
        return cutoff.to_string();
    }
    let seconds = duration.as_secs();
    [(24 * 60 * 60, "d"), (60 * 60, "h"), (60, "m")]
        .into_iter()
        .find(|(unit, _)| seconds != 0 && seconds % unit == 0)
        .map_or_else(
            || format!("{seconds}s"),
            |(unit, suffix)| format!("{}{suffix}", seconds / unit),
        )
}

/// Formats an absolute cutoff the way it is written to the manifest. A cutoff
/// at midnight UTC is written as the date of the day before, which is how a
/// date is parsed.
pub fn format_cutoff(cutoff: DateTime<Utc>) -> String {
    if cutoff.time() == NaiveTime::MIN
        && let Some(day) = cutoff.date_naive().pred_opt()
    {
        return day.format("%Y-%m-%d").to_string();
    }
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
        struct WithCutoff<'a, T> {
            #[serde(flatten)]
            spec: &'a T,
            cutoff: ExcludeNewer,
        }

        let Some(cutoff) = self.cutoff else {
            return self.spec.serialize(serializer);
        };
        match &self.spec {
            BinarySpec::Version(version) => WithCutoff {
                spec: &DetailedSpec {
                    version: Some(version.clone()),
                    ..DetailedSpec::default()
                },
                cutoff,
            }
            .serialize(serializer),
            BinarySpec::DetailedVersion(detailed) => WithCutoff {
                spec: &**detailed,
                cutoff,
            }
            .serialize(serializer),
            BinarySpec::Url(url) => WithCutoff { spec: url, cutoff }.serialize(serializer),
            BinarySpec::Path(path) => WithCutoff { spec: path, cutoff }.serialize(serializer),
        }
    }
}

/// An `exclude-newer` of the shared rattler configuration that is not valid
/// in a pixi manifest.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum ConfiguredExcludeNewerError {
    #[error("invalid exemption for `{}`", .name.as_source())]
    Exemption {
        name: PackageName,
        #[source]
        source: InvalidExemptionSpec,
    },
}

impl TryFrom<&rattler_config::config::exclude_newer::ExcludeNewerConfig> for ExcludeNewerConfig {
    type Error = ConfiguredExcludeNewerError;

    /// Converts the `exclude-newer` of the shared rattler configuration, which
    /// `rattler_config` has already validated.
    fn try_from(
        config: &rattler_config::config::exclude_newer::ExcludeNewerConfig,
    ) -> Result<Self, Self::Error> {
        let cutoff = ExcludeNewer::from(config.cutoff.value());
        // `rattler_config` resolves the channel of an exemption with the
        // default channel configuration, so its name is resolved with it too.
        let channel_config =
            ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap_or_default());
        let exemptions = config
            .exemptions
            .iter()
            .map(|(name, exemption)| {
                let exemption_error = |source| ConfiguredExcludeNewerError::Exemption {
                    name: name.clone(),
                    source,
                };
                let spec = match PixiSpec::from_nameless_matchspec(
                    exemption.spec.clone(),
                    &channel_config,
                ) {
                    PixiSpec::Version(version) => BinarySpec::Version(version),
                    PixiSpec::DetailedVersion(detailed) => BinarySpec::DetailedVersion(detailed),
                    // TODO: `rattler_config` accepts other match spec fields
                    // next to a `url`, e.g. `{ url = "…/ruff-0.1-0.conda",
                    // version = "==0.1" }`. They are dropped here, and pixi's
                    // manifest rejects them, so `pixi init` and `pixi global`
                    // write an exemption that the manifest can't parse, while
                    // `pixi exec` solves with it. `validate_exemption_spec` of
                    // `rattler_config` should reject a `url` with any field
                    // but `md5` and `sha256`: a `url` names one archive, so
                    // other fields are redundant or make the exemption match
                    // nothing, like the `url` of a file that isn't a package
                    // archive, which it rejects already. The configuration
                    // would then be rejected when it loads, for every command
                    // and tool.
                    PixiSpec::UrlBinary(url) => url.into(),
                    // `rattler_config` rejects a `url` that is not a package
                    // archive, which is what pixi reads as a source spec.
                    _ => return Err(exemption_error(InvalidExemptionSpec::NotBinary)),
                };
                let spec = validate_exemption_spec(name, spec).map_err(exemption_error)?;
                let cutoff = exemption
                    .cutoff
                    .as_ref()
                    .map(|cutoff| ExcludeNewer::from(cutoff.value()));
                Ok((name.clone(), ExcludeNewerExemption { spec, cutoff }))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            cutoff: Some(cutoff),
            exemptions,
        })
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
    resolve_exclude_newer_at(
        config,
        channels,
        channel_config,
        package_overrides,
        Utc::now(),
    )
}

/// Like [`resolve_exclude_newer`], with relative cutoffs resolved as if the
/// current time were `now`.
fn resolve_exclude_newer_at<'a>(
    config: &ExcludeNewerConfig,
    channels: impl IntoIterator<Item = &'a PrioritizedChannel>,
    channel_config: &ChannelConfig,
    package_overrides: impl IntoIterator<Item = (&'a PackageName, &'a ExcludeNewer)>,
    now: DateTime<Utc>,
) -> Result<Option<ResolvedExcludeNewer>, ExcludeNewerError> {
    let mut exclude_newer = config
        .cutoff
        .map(|cutoff| ResolvedExcludeNewer::from_datetime(cutoff.cutoff_at(now)));
    let mut configured = ConfiguredCutoffs {
        cutoff: config.cutoff,
        ..ConfiguredCutoffs::default()
    };

    for channel in channels {
        let Some(channel_exclude_newer) = channel.exclude_newer else {
            continue;
        };

        let channel_key = channel.channel.clone().into_base_url(channel_config)?;
        configured
            .channel_cutoffs
            .insert(channel_key.clone(), channel_exclude_newer);
        let config = exclude_newer
            .get_or_insert_with(|| ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC));
        *config = config
            .clone()
            .with_channel_cutoff(channel_key, channel_exclude_newer.cutoff_at(now));
    }

    #[allow(deprecated)]
    for (name, package_exclude_newer) in package_overrides {
        configured
            .package_cutoffs
            .insert(name.clone(), *package_exclude_newer);
        let config = exclude_newer
            .get_or_insert_with(|| ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC));
        *config = config
            .clone()
            .with_package_cutoff(name.clone(), package_exclude_newer.cutoff_at(now));
    }

    // Exemptions only matter when something is excluded in the first place.
    if let Some(mut resolved) = exclude_newer.take() {
        for (name, exemption) in &config.exemptions {
            let spec = exemption_match_spec(name, exemption, channel_config)?;
            if let Some(cutoff) = exemption.cutoff {
                configured.exemption_cutoffs.push((spec.clone(), cutoff));
            }
            resolved = resolved
                .with_exemption_until(spec, exemption.cutoff.map(|cutoff| cutoff.cutoff_at(now)))?;
        }
        resolved = resolved.with_configured_cutoffs(configured);
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
/// with a cutoff that do not exempt anything, because every cutoff they can
/// apply to is already at or past the cutoff of the exemption.
///
/// `environments` are the channels of every environment the exemptions apply
/// to. Each environment is resolved like for its solves, see
/// [`resolve_exclude_newer`]. This includes the default cutoff, which applies
/// to the channels of e.g. build backends that are not channels of the
/// environment. `package_overrides` are the per-package cutoffs of the
/// deprecated top-level `[exclude-newer]` table.
///
/// A stale exemption stays stale unless the configuration changes. Relative
/// cutoffs only move forward in time, so an exemption with a point in time as
/// its cutoff that is stale now stays stale. An exemption with a duration
/// moves forward as well, and eventually past every point in time, so it is
/// only stale if it still is at the end of time.
pub fn stale_exemptions<'a, Channels>(
    config: &ExcludeNewerConfig,
    environments: impl IntoIterator<Item = Channels>,
    channel_config: &ChannelConfig,
    package_overrides: impl IntoIterator<Item = (&'a PackageName, &'a ExcludeNewer)> + Clone,
    now: DateTime<Utc>,
) -> Result<Vec<PackageName>, ExcludeNewerError>
where
    Channels: IntoIterator<Item = &'a PrioritizedChannel>,
{
    let environments = environments
        .into_iter()
        .map(|channels| channels.into_iter().collect_vec())
        .collect_vec();
    let resolve_at = |now: DateTime<Utc>| {
        let mut resolved = environments
            .iter()
            .map(|channels| {
                resolve_exclude_newer_at(
                    config,
                    channels.iter().copied(),
                    channel_config,
                    package_overrides.clone(),
                    now,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Without environments, the exemptions apply to the ones added later.
        if resolved.is_empty() {
            resolved.push(resolve_exclude_newer_at(
                config,
                [],
                channel_config,
                package_overrides.clone(),
                now,
            )?);
        }
        Ok::<_, ExcludeNewerError>(resolved)
    };
    let resolved_now = resolve_at(now)?;
    let end_of_time = DateTime::<Utc>::MAX_UTC;
    let resolved_at_end = if config
        .exemptions
        .values()
        .any(|exemption| matches!(exemption.cutoff, Some(ExcludeNewer::Duration(_))))
    {
        resolve_at(end_of_time)?
    } else {
        Vec::new()
    };

    let mut stale = Vec::new();
    for (name, exemption) in &config.exemptions {
        let Some(cutoff) = exemption.cutoff else {
            continue;
        };
        let (resolved, at) = match cutoff {
            ExcludeNewer::Timestamp(_) => (&resolved_now, now),
            ExcludeNewer::Duration(_) => (&resolved_at_end, end_of_time),
        };
        let cutoff = cutoff.cutoff_at(at);
        let spec = exemption_match_spec(name, exemption, channel_config)?;
        let is_stale = resolved.iter().all(|resolved| {
            // Without a resolved configuration nothing is excluded.
            let Some(resolved) = resolved else {
                return false;
            };
            let effective_cutoff = match &spec.channel {
                Some(channel) => resolved.cutoff_for(name, &channel.base_url),
                None => resolved.earliest_cutoff_for(name),
            };
            cutoff <= effective_cutoff
        });
        if is_stale {
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

    for (name, package_exclude_newer) in package_overrides {
        // A zero cutoff such as `0d` lifts the cutoff for the package, which
        // is what an exemption does. As an exemption, it also keeps releases
        // without an upload time, which a cutoff of now would exclude.
        exclude_newer = if matches!(package_exclude_newer, ExcludeNewer::Duration(d) if d.is_zero())
        {
            exclude_newer.with_exempt_package(name.as_normalized().clone())
        } else {
            #[allow(deprecated)]
            exclude_newer
                .with_package_cutoff(name.as_normalized().clone(), package_exclude_newer.cutoff())
        };
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
        stale_in_environments(config, &[channels])
    }

    fn stale_in_environments(
        config: &ExcludeNewerConfig,
        environments: &[&[PrioritizedChannel]],
    ) -> Vec<String> {
        stale_exemptions(
            config,
            environments.iter().copied(),
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
    fn test_stale_exemptions_duration_cutoff() {
        let config = ExcludeNewerConfig {
            cutoff: Some(ExcludeNewer::from_str("7d").unwrap()),
            exemptions: IndexMap::from_iter([
                ("longer".parse().unwrap(), exemption("==1", Some("14d"))),
                ("equal".parse().unwrap(), exemption("==1", Some("7d"))),
                ("shorter".parse().unwrap(), exemption("==1", Some("1d"))),
            ]),
        };
        assert_eq!(
            stale(&config, &[channel("conda-forge", None)]),
            ["longer", "equal"]
        );

        // A duration moves past a point in time, so an exemption that is
        // stale now but not later is kept.
        let config = ExcludeNewerConfig {
            cutoff: Some(ExcludeNewer::from_str("2026-10-28T00:00:00Z").unwrap()),
            exemptions: IndexMap::from_iter([
                ("relative".parse().unwrap(), exemption("==1", Some("2d"))),
                (
                    "absolute".parse().unwrap(),
                    exemption("==1", Some("2026-10-25T00:00:00Z")),
                ),
            ]),
        };
        assert_eq!(
            stale(&config, &[channel("conda-forge", None)]),
            ["absolute"]
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

    #[test]
    fn test_stale_exemptions_default_cutoff() {
        let config = ExcludeNewerConfig {
            // 2026-09-27T00:00:00Z
            cutoff: Some(ExcludeNewer::from_str("30d").unwrap()),
            exemptions: IndexMap::from_iter([(
                "openssl".parse().unwrap(),
                exemption("==3.2.1", Some("2026-10-22T00:00:00Z")),
            )]),
        };
        // The default cutoff still applies to the channels of e.g. build
        // backends, so the exemption is not stale even though the cutoff of
        // every channel of the environment is past it.
        assert_eq!(
            stale(&config, &[channel("conda-forge", Some("1d"))]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_stale_exemptions_of_environments() {
        let config = ExcludeNewerConfig {
            cutoff: None,
            exemptions: IndexMap::from_iter([(
                "polars".parse().unwrap(),
                exemption("conda-forge::==1", Some("2026-10-19T00:00:00Z")),
            )]),
        };
        let recent = [channel("conda-forge", Some("2026-10-20T00:00:00Z"))];
        let old = [channel("conda-forge", Some("2026-10-18T00:00:00Z"))];
        assert_eq!(stale(&config, &recent), ["polars"]);
        // An exemption is only stale once it is stale for every environment.
        assert_eq!(
            stale_in_environments(&config, &[&recent, &old]),
            Vec::<String>::new()
        );
        // Without a cutoff nothing is excluded, so nothing is stale.
        assert_eq!(stale(&config, &[]), Vec::<String>::new());
    }

    /// Relative cutoffs resolve to a different time on every invocation,
    /// which must not change the configured hash that on-disk cache keys use.
    #[test]
    fn test_configured_hash_is_stable_for_relative_cutoffs() {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let config = ExcludeNewerConfig {
            cutoff: Some(ExcludeNewer::from_str("14d").unwrap()),
            exemptions: IndexMap::from_iter([
                (
                    "polars".parse().unwrap(),
                    exemption("==1", Some("2026-10-19T00:00:00Z")),
                ),
                ("ruff".parse().unwrap(), exemption("==1", Some("3d"))),
            ]),
        };
        let channels = [channel("conda-forge", Some("7d"))];
        let configured_hash = |config: &ExcludeNewerConfig, now: &str| {
            let resolved = resolve_exclude_newer_at(
                config,
                &channels,
                &ChannelConfig::default_with_root_dir("/".into()),
                [],
                now.parse().unwrap(),
            )
            .unwrap()
            .unwrap();
            let mut hasher = DefaultHasher::new();
            resolved.hash_configured(&mut hasher);
            let mut resolved_hasher = DefaultHasher::new();
            resolved.hash(&mut resolved_hasher);
            (hasher.finish(), resolved_hasher.finish())
        };

        let (early, early_resolved) = configured_hash(&config, "2026-10-01T00:00:00Z");
        let (late, late_resolved) = configured_hash(&config, "2026-10-28T00:00:00Z");
        assert_ne!(early_resolved, late_resolved);
        assert_eq!(early, late);

        let other = ExcludeNewerConfig {
            cutoff: Some(ExcludeNewer::from_str("7d").unwrap()),
            ..config.clone()
        };
        assert_ne!(early, configured_hash(&other, "2026-10-01T00:00:00Z").0);
    }

    #[test]
    fn test_resolve_pypi_zero_package_cutoff_is_exemption() {
        let config = PypiExcludeNewerConfig {
            cutoff: Some(ExcludeNewer::from_str("2026-10-20T00:00:00Z").unwrap()),
            exemptions: IndexMap::new(),
        };
        let zero: PypiPackageName = "zero".parse().unwrap();
        let dated: PypiPackageName = "dated".parse().unwrap();
        let overrides = [
            (zero.clone(), ExcludeNewer::from_str("0d").unwrap()),
            (
                dated.clone(),
                ExcludeNewer::from_str("2026-10-25T00:00:00Z").unwrap(),
            ),
        ];

        let resolved =
            resolve_pypi_exclude_newer(&config, None, overrides.iter().map(|(n, c)| (n, c)));

        assert_eq!(
            resolved.exempt_packages.iter().collect::<Vec<_>>(),
            [zero.as_normalized()]
        );
        assert_eq!(
            resolved.package_cutoffs.keys().collect::<Vec<_>>(),
            [dated.as_normalized()]
        );
    }

    /// Parses `toml`, the value of `exclude-newer` in the shared rattler
    /// configuration.
    fn configured(toml: &str) -> rattler_config::config::exclude_newer::ExcludeNewerConfig {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct Document {
            exclude_newer: rattler_config::config::exclude_newer::ExcludeNewerConfig,
        }
        toml_edit::de::from_str::<Document>(&format!("exclude-newer = {toml}"))
            .unwrap()
            .exclude_newer
    }

    #[test]
    fn test_from_configured_exclude_newer() {
        let config = ExcludeNewerConfig::try_from(&configured(
            r#"{ cutoff = "7d", exemptions = { ruff = ">=0.5", polars = { version = "==1.43.1", build = "py3*", channel = "conda-forge", cutoff = "2026-10-12" } } }"#,
        ))
        .unwrap();
        assert_eq!(
            config.cutoff,
            Some(ExcludeNewer::Duration(std::time::Duration::from_secs(
                7 * 24 * 60 * 60
            )))
        );
        assert_eq!(
            config.exemptions[&PackageName::new_unchecked("ruff")],
            ExcludeNewerExemption {
                spec: BinarySpec::Version(
                    rattler_conda_types::VersionSpec::from_str(
                        ">=0.5",
                        rattler_conda_types::ParseStrictness::Strict
                    )
                    .unwrap()
                ),
                cutoff: None,
            }
        );
        let polars = &config.exemptions[&PackageName::new_unchecked("polars")];
        let BinarySpec::DetailedVersion(spec) = &polars.spec else {
            panic!("expected a detailed spec, got {:?}", polars.spec);
        };
        assert_eq!(spec.version.as_ref().unwrap().to_string(), "==1.43.1");
        assert_eq!(spec.build.as_ref().unwrap().to_string(), "py3*");
        assert_eq!(spec.channel.as_ref().unwrap().to_string(), "conda-forge");
        assert_eq!(
            polars.cutoff.map(format_exclude_newer).as_deref(),
            Some("2026-10-12")
        );
    }

    #[test]
    fn test_from_configured_exclude_newer_url_exemption() {
        let url = "https://conda.anaconda.org/conda-forge/noarch/ruff-0.1-0.conda";
        let config = ExcludeNewerConfig::try_from(&configured(&format!(
            r#"{{ cutoff = "7d", exemptions = {{ ruff = {{ url = "{url}", cutoff = "2026-10-12" }} }} }}"#
        )))
        .unwrap();
        let ruff = &config.exemptions[&PackageName::new_unchecked("ruff")];
        let BinarySpec::Url(spec) = &ruff.spec else {
            panic!("expected a url spec, got {:?}", ruff.spec);
        };
        assert_eq!(spec.url.as_str(), url);
        assert_eq!(
            ruff.to_toml_value().to_string(),
            format!(r#"{{ url = "{url}", cutoff = "2026-10-12" }}"#)
        );
    }

    #[test]
    fn test_from_configured_exclude_newer_duration_exemption_cutoff() {
        let config = ExcludeNewerConfig::try_from(&configured(
            r#"{ cutoff = "7d", exemptions = { ruff = { version = "*", cutoff = "36h" } } }"#,
        ))
        .unwrap();
        let ruff = &config.exemptions[&PackageName::new_unchecked("ruff")];
        assert_eq!(
            ruff.to_toml_value().to_string(),
            r#"{ version = "*", cutoff = "36h" }"#
        );
    }

    /// A `url` that is not a package archive can't be exempted, which
    /// `rattler_config` already rejects.
    #[test]
    fn test_configured_exclude_newer_rejects_url_exemption() {
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct Document {
            #[expect(dead_code)]
            exclude_newer: rattler_config::config::exclude_newer::ExcludeNewerConfig,
        }
        let error = toml_edit::de::from_str::<Document>(
            r#"exclude-newer = { cutoff = "7d", exemptions = { ruff = { url = "https://example.com/ruff" } } }"#,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the `url` of an exemption must point to a package archive"),
            "{error}"
        );
    }
}
