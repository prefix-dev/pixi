use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use url::Url;

use crate::{CompressedMapping, ProjectDefinedMapping};

pub type ChannelName = String;
pub type MappingMap = HashMap<ChannelName, ProjectDefinedChannelMapping>;
pub type MappingByChannel = HashMap<ChannelName, ResolvedChannelMapping>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDefinedMappingLocation {
    Path(PathBuf),
    Url {
        url: Url,
        /// When set, the fetched mapping is cached on disk and only
        /// re-fetched once the cached copy is older than this duration.
        cache_ttl: Option<Duration>,
    },
    InMemory(CompressedMapping),
}

/// How a project-defined channel mapping interacts with the default
/// prefix.dev derivation chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MappingMode {
    /// The mapping overlays the defaults: a hit (including an explicit "not a
    /// PyPI package" entry) is final, a miss falls through to the prefix.dev
    /// chain (hash mapping, compressed mapping, conda-forge verbatim
    /// fallback).
    #[default]
    Extend,
    /// The mapping is exclusive: only packages in the mapping get purls. No
    /// prefix.dev lookups and no conda-forge verbatim fallback happen for
    /// records from this channel.
    Replace,
    /// No purls are looked up for records from this channel, neither
    /// project-defined nor prefix.dev. The offline conda-forge verbatim
    /// fallback still applies.
    Disabled,
}

/// The project-defined mapping configuration for a single channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDefinedChannelMapping {
    /// The mapping sources, merged in order: entries from later sources
    /// override entries from earlier ones.
    pub sources: Vec<ProjectDefinedMappingLocation>,
    pub mode: MappingMode,
}

impl ProjectDefinedChannelMapping {
    pub fn new(sources: Vec<ProjectDefinedMappingLocation>, mode: MappingMode) -> Self {
        Self { sources, mode }
    }

    /// A single-source mapping that overlays the prefix.dev defaults.
    pub fn extend(source: ProjectDefinedMappingLocation) -> Self {
        Self::new(vec![source], MappingMode::Extend)
    }

    /// A single-source mapping that replaces the prefix.dev defaults.
    pub fn replace(source: ProjectDefinedMappingLocation) -> Self {
        Self::new(vec![source], MappingMode::Replace)
    }

    /// Disable purl lookups for this channel.
    pub fn disabled() -> Self {
        Self::new(Vec::new(), MappingMode::Disabled)
    }
}

/// A channel mapping with all its sources fetched and merged.
#[derive(Debug, Clone)]
pub struct ResolvedChannelMapping {
    pub mapping: CompressedMapping,
    pub mode: MappingMode,
}

/// User-selected mapping mode.
///
/// This controls which resolver family [`crate::PurlDerivationClient`] uses. It is not
/// the same thing as [`crate::PurlDerivationSource`], which identifies the
/// concrete resolver that produced an individual purl.
#[derive(Debug, Clone)]
pub enum PurlDerivationMode {
    /// Use project-defined per-channel mappings. Depending on each channel's
    /// [`MappingMode`] the prefix.dev mappings may still be used as a
    /// fallback. Records from channels without a project-defined mapping use
    /// the prefix.dev mappings.
    ProjectDefined(Arc<ProjectDefinedMapping>),
    /// Use prefix.dev mappings: hash mapping first, then compressed mapping.
    Prefix,
    /// Disable project-defined and prefix.dev mappings.
    ///
    /// The offline conda-forge verbatim fallback (assume the conda name is
    /// the PyPI name) still applies in this mode; disabling only turns off
    /// the lookups.
    Disabled,
}

impl PurlDerivationMode {
    /// Return the project-defined mapping
    /// for `PurlDerivationMode::ProjectDefined`
    pub fn project_defined(&self) -> Option<Arc<ProjectDefinedMapping>> {
        match self {
            PurlDerivationMode::ProjectDefined(mapping) => Some(mapping.clone()),
            _ => None,
        }
    }
}
