//! Converts the manifest `[workspace.conda-pypi-map]` configuration into the
//! per-channel mapping configuration used by the purl derivation client.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
};

use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::{
    CondaPypiMap, CondaPypiMapEntry, CondaPypiMapSpec, CondaPypiMappingMode, WorkspaceManifest,
};
use pypi_mapping::{
    ChannelName, MappingMode, ProjectDefinedChannelMapping, ProjectDefinedMapping,
    ProjectDefinedMappingLocation, PurlDerivationMode, PypiNames, is_conda_forge_url,
};
use rattler_conda_types::{Channel, ChannelConfig, NamedChannelOrUrl, ParseChannelError};
use rattler_lock::UrlOrPath;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum CondaPypiMapError {
    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),

    #[error("channel '{0}' is configured more than once in `conda-pypi-map`")]
    #[diagnostic(help("Keep a single entry per channel, e.g. not both by name and by URL."))]
    DuplicateChannel(String),

    #[error("channel '{channel}' in `conda-pypi-map` is not a workspace or feature channel")]
    #[diagnostic(help("The channels are: {channels}"))]
    UnusedChannel { channel: String, channels: String },

    #[error("failed to parse mapping location `{location}`")]
    InvalidLocation {
        location: String,
        #[source]
        source: <UrlOrPath as FromStr>::Err,
    },

    #[error("unsupported scheme `{scheme}` in mapping location `{location}`")]
    #[diagnostic(help("Only http(s) URLs and local paths are supported."))]
    UnsupportedScheme { scheme: String, location: String },
}

/// Determine the [`PurlDerivationMode`] for a workspace from its
/// `conda-pypi-map` configuration.
pub(crate) fn build_pypi_name_derivation_mode(
    manifest: &WorkspaceManifest,
    channel_config: &ChannelConfig,
) -> Result<PurlDerivationMode, CondaPypiMapError> {
    let map = match &manifest.workspace.conda_pypi_map {
        None => return Ok(PurlDerivationMode::Prefix),
        Some(CondaPypiMap::Disabled) => return Ok(PurlDerivationMode::Disabled),
        Some(CondaPypiMap::Map(map)) => map,
    };

    // An empty map is a soft-deprecated legacy spelling for "avoid the
    // network/default mapping data, but keep the conda-forge same-name
    // heuristic". Preserve that behavior while encouraging an explicit
    // per-channel replacement mapping in the parse warning.
    if map.is_empty() {
        return Ok(PurlDerivationMode::ProjectDefined(
            ProjectDefinedMapping::new(HashMap::from([(
                conda_forge_channel_name(channel_config)?,
                ProjectDefinedChannelMapping::new(Vec::new(), MappingMode::Replace, true),
            )]))
            .into(),
        ));
    }

    // The manifest map can spell the same channel in different forms (by
    // name and by URL) that only collapse once resolved to a `Channel`.
    // Collecting blindly would keep a nondeterministic winner, so reject
    // duplicates instead.
    let mut channel_to_entry_map: HashMap<Channel, &CondaPypiMapEntry> =
        HashMap::with_capacity(map.len());
    for (key, value) in map {
        let channel = key.clone().into_channel(channel_config)?;
        let channel_name = channel
            .name
            .clone()
            .unwrap_or_else(|| channel.base_url.to_string());
        if channel_to_entry_map.insert(channel, value).is_some() {
            return Err(CondaPypiMapError::DuplicateChannel(channel_name));
        }
    }

    validate_mapped_channels_are_used(manifest, channel_config, channel_to_entry_map.keys())?;

    let mapping = channel_to_entry_map
        .iter()
        .map(|(channel, entry)| {
            Ok((
                channel.canonical_name().trim_end_matches('/').into(),
                convert_entry(entry, channel, channel_config)?,
            ))
        })
        .collect::<Result<HashMap<_, _>, CondaPypiMapError>>()?;

    Ok(PurlDerivationMode::ProjectDefined(
        ProjectDefinedMapping::new(mapping).into(),
    ))
}

fn conda_forge_channel_name(
    channel_config: &ChannelConfig,
) -> Result<ChannelName, ParseChannelError> {
    let channel =
        NamedChannelOrUrl::Name("conda-forge".to_string()).into_channel(channel_config)?;
    Ok(channel.canonical_name().trim_end_matches('/').into())
}

/// Every channel in `conda-pypi-map` must appear in the workspace or feature
/// channels; an entry for an unused channel is almost certainly a typo.
fn validate_mapped_channels_are_used<'a>(
    manifest: &WorkspaceManifest,
    channel_config: &ChannelConfig,
    mapped_channels: impl Iterator<Item = &'a Channel>,
) -> Result<(), CondaPypiMapError> {
    let project_channels: HashSet<_> = manifest
        .workspace
        .channels
        .iter()
        .map(|pc| pc.channel.clone().into_channel(channel_config))
        .try_collect()?;

    let feature_channels: HashSet<_> = manifest
        .all_features()
        .flat_map(|(_, feature)| feature.channels.iter())
        .flatten()
        .map(|pc| pc.channel.clone().into_channel(channel_config))
        .try_collect()?;

    let project_and_feature_channels: HashSet<_> =
        project_channels.union(&feature_channels).collect();

    for channel in mapped_channels {
        if !project_and_feature_channels.contains(channel) {
            let channels = project_and_feature_channels
                .iter()
                .map(|c| c.name.clone().unwrap_or_else(|| c.base_url.to_string()))
                .sorted()
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CondaPypiMapError::UnusedChannel {
                channel: channel
                    .name
                    .clone()
                    .unwrap_or_else(|| channel.base_url.to_string()),
                channels,
            });
        }
    }
    Ok(())
}

/// Convert a manifest entry to the per-channel mapping configuration used by
/// the purl derivation client.
fn convert_entry(
    entry: &CondaPypiMapEntry,
    channel: &Channel,
    channel_config: &ChannelConfig,
) -> Result<ProjectDefinedChannelMapping, CondaPypiMapError> {
    match entry {
        CondaPypiMapEntry::Disabled => Ok(ProjectDefinedChannelMapping::disabled()),
        CondaPypiMapEntry::Map(CondaPypiMapSpec {
            location,
            mapping,
            mapping_mode,
            same_name_heuristic,
        }) => {
            let mut sources = Vec::new();
            if let Some(location) = location {
                sources.push(parse_mapping_location(location, channel_config)?);
            }
            // Inline entries come last so they override entries from the
            // location. Keys are lowercased to match the normalized conda
            // package names used for lookups.
            if let Some(inline) = mapping {
                sources.push(ProjectDefinedMappingLocation::InMemory(
                    inline
                        .iter()
                        .map(|(name, pypi_names)| {
                            (name.to_lowercase(), PypiNames(pypi_names.clone()))
                        })
                        .collect(),
                ));
            }
            Ok(ProjectDefinedChannelMapping::new(
                sources,
                convert_mode(*mapping_mode),
                same_name_heuristic.unwrap_or_else(|| {
                    url::Url::parse(channel.base_url.as_str())
                        .is_ok_and(|url| is_conda_forge_url(&url))
                }),
            ))
        }
    }
}

/// Convert the manifest-level mapping mode to the derivation-level [`MappingMode`].
///
/// The two enums are deliberately asymmetric: `MappingMode::Disabled` has no
/// manifest-level mode string because "disabled" is spelled `<channel> =
/// false` in TOML (see [`CondaPypiMapEntry::Disabled`]). This function and the
/// `Disabled` arm in [`convert_entry`] are the single place where the two
/// representations meet. (A `From` impl cannot encode this: neither
/// `pixi_manifest` nor `pypi_mapping` depends on the other, so the orphan rule
/// forces the conversion to live here.)
fn convert_mode(mode: CondaPypiMappingMode) -> MappingMode {
    match mode {
        CondaPypiMappingMode::Overlay => MappingMode::Overlay,
        CondaPypiMappingMode::Replace => MappingMode::Replace,
    }
}

/// Classify a manifest location spec into a url or a path, resolving relative
/// paths against the workspace root. `file://` urls are normalized to paths.
fn parse_mapping_location(
    location: &str,
    channel_config: &ChannelConfig,
) -> Result<ProjectDefinedMappingLocation, CondaPypiMapError> {
    let url_or_path =
        UrlOrPath::from_str(location).map_err(|source| CondaPypiMapError::InvalidLocation {
            location: location.to_string(),
            source,
        })?;

    match url_or_path {
        UrlOrPath::Url(url) => {
            if !matches!(url.scheme(), "http" | "https") {
                return Err(CondaPypiMapError::UnsupportedScheme {
                    scheme: url.scheme().to_string(),
                    location: location.to_string(),
                });
            }
            // A plaintext mapping URL can be tampered with on the network,
            // and a tampered mapping changes which conda packages are
            // considered to satisfy PyPI dependencies.
            if url.scheme() == "http" {
                tracing::warn!(
                    "the conda-pypi mapping location `{}` uses plain `http://`; the mapping can \
                     be tampered with in transit. Prefer `https://` or a local file.",
                    location
                );
            }
            Ok(ProjectDefinedMappingLocation::Url { url })
        }
        UrlOrPath::Path(path) => {
            let path = PathBuf::from(path.as_str());
            let abs_path = if path.is_relative() {
                channel_config.root_dir.join(path)
            } else {
                path
            };
            Ok(ProjectDefinedMappingLocation::Path(abs_path))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn channel_config() -> ChannelConfig {
        ChannelConfig::default_with_root_dir(PathBuf::from("/workspace"))
    }

    #[test]
    fn test_parse_mapping_location_http_url() {
        let parsed =
            parse_mapping_location("https://example.com/m.json", &channel_config()).unwrap();
        assert_eq!(
            parsed,
            ProjectDefinedMappingLocation::Url {
                url: "https://example.com/m.json".parse().unwrap(),
            }
        );
    }

    #[test]
    fn test_parse_mapping_location_relative_path() {
        let parsed = parse_mapping_location("sub/m.json", &channel_config()).unwrap();
        assert_eq!(
            parsed,
            ProjectDefinedMappingLocation::Path(PathBuf::from("/workspace/sub/m.json"))
        );
    }

    #[test]
    fn test_parse_mapping_location_file_url_becomes_path() {
        let parsed = parse_mapping_location("file:///abs/m.json", &channel_config()).unwrap();
        assert_eq!(
            parsed,
            ProjectDefinedMappingLocation::Path(PathBuf::from("/abs/m.json"))
        );
    }

    #[test]
    fn test_parse_mapping_location_rejects_unsupported_scheme() {
        let err =
            parse_mapping_location("ftp://example.com/m.json", &channel_config()).unwrap_err();
        assert!(matches!(err, CondaPypiMapError::UnsupportedScheme { .. }));
    }

    #[test]
    fn test_convert_entry_disabled() {
        let channel = Channel::from_str("conda-forge", &channel_config()).unwrap();
        let converted =
            convert_entry(&CondaPypiMapEntry::Disabled, &channel, &channel_config()).unwrap();
        assert_eq!(converted, ProjectDefinedChannelMapping::disabled());
    }
}
