use std::fmt::Write;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_pypi_spec::{PypiPackageName, VersionOrStar};
use pixi_spec::{
    BinarySpec, ExcludeNewer, InvalidExemptionError, ResolvedExcludeNewer, SpecConversionError,
};
use rattler_conda_types::{ChannelConfig, PackageName, ParseChannelError};

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExcludeNewerConfig {
    /// Packages uploaded after this cutoff are excluded from the solve.
    pub cutoff: Option<ExcludeNewer>,

    /// Package releases that are never excluded, regardless of their
    /// timestamp. The value is a binary spec, so `polars = "==1.43.1"`
    /// exempts that release only and `py-rattler = "*"` exempts every
    /// release.
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
    /// package, which is exactly what the wildcard exemption `"*"` does. Any
    /// other per-package cutoff has no replacement and will stop working once
    /// the table is removed.
    pub fn new<'a>(
        table: &str,
        cutoff: Option<ExcludeNewer>,
        overrides: impl IntoIterator<Item = (&'a str, &'a ExcludeNewer)>,
    ) -> Self {
        let (exemptions, without_replacement): (Vec<_>, Vec<_>) = overrides
            .into_iter()
            .partition(|(_, cutoff)| matches!(cutoff, ExcludeNewer::Duration(d) if d.is_zero()));

        // Conda exemptions require a cutoff in `[workspace.exclude-newer]`,
        // while PyPI exemptions fall back to it.
        if table == "exclude-newer" && cutoff.is_none() && !exemptions.is_empty() {
            let packages = exemptions
                .iter()
                .chain(&without_replacement)
                .map(|(package, _)| format!("`{package}`"))
                .join(", ");
            return Self {
                has_replacement: false,
                help: format!(
                    "set a `cutoff` in `[workspace.{table}]` to replace zero cutoffs with `exemptions`, the cutoffs of {packages} will stop working when the table is removed in a future release"
                ),
            };
        }

        let mut help = String::new();
        if !exemptions.is_empty() {
            let entries = if without_replacement.is_empty() {
                "the entries"
            } else {
                "the entries with a zero cutoff"
            };
            let _ = write!(
                help,
                "move {entries} to the `exemptions` of `[workspace.{table}]` instead, e.g.\n\n[workspace.{table}]"
            );
            if let Some(cutoff) = cutoff {
                let _ = write!(help, "\ncutoff = \"{cutoff}\"");
            }
            let exemptions = exemptions
                .iter()
                .map(|(package, _)| format!("{package} = \"*\""))
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
            let packages = without_replacement
                .iter()
                .map(|(package, _)| format!("`{package}`"))
                .join(", ");
            let _ = write!(
                help,
                "per-package cutoffs other than a zero cutoff have no replacement, the cutoffs of {packages} will stop working when the table is removed in a future release"
            );
        }

        Self {
            has_replacement: !exemptions.is_empty(),
            help,
        }
    }
}
