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

/// How a project-defined channel mapping interacts with Pixi's default
/// mapping data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MappingMode {
    /// The project mapping overlays Pixi's default mapping data: project
    /// entries win, and misses fall through to the prefix.dev chain.
    #[default]
    Overlay,
    /// The project mapping replaces Pixi's default mapping data. The
    /// same-name heuristic is controlled separately.
    Replace,
    /// No purls are looked up for records from this channel.
    Disabled,
}

/// The project-defined mapping configuration for a single channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDefinedChannelMapping {
    /// The mapping sources, merged in order: entries from later sources
    /// override entries from earlier ones.
    pub sources: Vec<ProjectDefinedMappingLocation>,
    pub mode: MappingMode,
    pub same_name: bool,
}

impl ProjectDefinedChannelMapping {
    pub fn new(
        sources: Vec<ProjectDefinedMappingLocation>,
        mode: MappingMode,
        same_name: bool,
    ) -> Self {
        Self {
            sources,
            mode,
            same_name,
        }
    }

    /// A single-source mapping that overlays the prefix.dev defaults.
    pub fn overlay(source: ProjectDefinedMappingLocation) -> Self {
        Self::new(vec![source], MappingMode::Overlay, true)
    }

    /// Backwards-compatible constructor used by tests and callers.
    pub fn extend(source: ProjectDefinedMappingLocation) -> Self {
        Self::overlay(source)
    }

    /// A single-source mapping that replaces the prefix.dev defaults.
    pub fn replace(source: ProjectDefinedMappingLocation) -> Self {
        Self::new(vec![source], MappingMode::Replace, true)
    }

    /// Disable purl lookups for this channel.
    pub fn disabled() -> Self {
        Self::new(Vec::new(), MappingMode::Disabled, false)
    }
}

/// A channel mapping with all its sources fetched and merged.
#[derive(Debug, Clone)]
pub struct ResolvedChannelMapping {
    pub mapping: CompressedMapping,
    pub mode: MappingMode,
    pub same_name: bool,
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
    /// Disable project-defined, prefix.dev, and same-name mappings.
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
