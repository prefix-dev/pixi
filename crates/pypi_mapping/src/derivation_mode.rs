use std::{collections::HashMap, path::PathBuf, sync::Arc};

use url::Url;

use crate::{CompressedMapping, ProjectDefinedMapping};

pub type ChannelName = String;
pub type MappingMap = HashMap<ChannelName, ProjectDefinedMappingLocation>;
pub type MappingByChannel = HashMap<ChannelName, CompressedMapping>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDefinedMappingLocation {
    Path(PathBuf),
    Url(Url),
    InMemory(CompressedMapping),
}

/// User-selected mapping mode.
///
/// This controls which resolver family [`crate::PurlDerivationClient`] uses. It is not
/// the same thing as [`crate::PurlDerivationSource`], which identifies the
/// concrete resolver that produced an individual purl.
#[derive(Debug, Clone)]
pub enum PurlDerivationMode {
    /// Use only project-defined per-channel mappings.
    ProjectDefined(Arc<ProjectDefinedMapping>),
    /// Use prefix.dev mappings: hash mapping first, then compressed mapping.
    Prefix,
    /// Disable project-defined and prefix.dev mappings.
    ///
    /// Note: the current resolver still allows the conda-forge verbatim fallback
    /// in this mode.
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
