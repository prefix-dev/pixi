use std::fmt::Write;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use miette::Diagnostic;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{
    BinarySpec, ExcludeNewer, InvalidExemptionError, ResolvedExcludeNewer, SpecConversionError,
};
use rattler_conda_types::{ChannelConfig, PackageName, ParseChannelError};
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
/// exemptions = { py-rattler = "*", polars = "1.43.1" }
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExcludeNewerConfig {
    /// Packages uploaded after this cutoff are excluded from the solve.
    pub cutoff: Option<ExcludeNewer>,

    /// Package releases that are never excluded, regardless of their
    /// timestamp. The value is a binary spec, so `polars = "1.43.1"` exempts
    /// that release only and `py-rattler = "*"` exempts every release.
    pub exemptions: IndexMap<PackageName, BinarySpec>,
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

impl serde::Serialize for PypiExcludeNewerConfig {
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
        for (name, spec) in &config.exemptions {
            let spec = spec
                .clone()
                .to_match_spec(name, channel_config)
                .map_err(|source| ExcludeNewerError::Exemption {
                    name: name.clone(),
                    source,
                })?;
            resolved = resolved.with_exemption(spec)?;
        }
        exclude_newer = Some(resolved);
    }

    Ok(exclude_newer)
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

    for name in config.exemptions.keys() {
        exclude_newer = exclude_newer.with_exempt_package(name.as_normalized().clone());
    }

    exclude_newer
}

/// Renders the exemptions that replace the deprecated per-package cutoffs of
/// a top-level `[exclude-newer]` or `[pypi-exclude-newer]` table, for use in
/// the deprecation hint.
///
/// A per-package cutoff lowers the cutoff for every release of the package,
/// which the wildcard exemption `"*"` comes closest to. Conda users are
/// encouraged to narrow it down to the releases they vetted; PyPI exemptions
/// only support the wildcard for now.
pub(crate) fn deprecated_table_migration_help<'a>(
    table: &str,
    cutoff: Option<ExcludeNewer>,
    packages: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut help = format!(
        "move the entries to the `exemptions` of `[workspace.{table}]` instead, e.g.\n\n[workspace.{table}]"
    );
    if let Some(cutoff) = cutoff {
        let _ = write!(help, "\ncutoff = \"{cutoff}\"");
    }
    let exemptions = packages
        .into_iter()
        .map(|package| format!("{package} = \"*\""))
        .collect::<Vec<_>>()
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
    help
}
