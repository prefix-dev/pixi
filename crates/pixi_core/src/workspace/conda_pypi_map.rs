//! Converts the manifest `[workspace.conda-pypi-map]` configuration into the
//! per-channel mapping configuration used by the purl derivation client.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_manifest::{
    CondaPypiMap, CondaPypiMapEntry, CondaPypiMapMode, CondaPypiMapSpec, MappingLocationSpec,
    WorkspaceManifest,
};
use pypi_mapping::{
    ChannelName, MappingMode, ProjectDefinedChannelMapping, ProjectDefinedMapping,
    ProjectDefinedMappingLocation, PurlDerivationMode,
};
use rattler_conda_types::{Channel, ChannelConfig};
use rattler_lock::UrlOrPath;

/// Determine the [`PurlDerivationMode`] for a workspace from its
/// `conda-pypi-map` configuration.
pub(crate) fn build_pypi_name_derivation_mode(
    manifest: &WorkspaceManifest,
    channel_config: &ChannelConfig,
) -> miette::Result<PurlDerivationMode> {
    let map = match &manifest.workspace.conda_pypi_map {
        None => return Ok(PurlDerivationMode::Prefix),
        Some(CondaPypiMap::Disabled) => return Ok(PurlDerivationMode::Disabled),
        Some(CondaPypiMap::Map(map)) => map,
    };

    // An empty map is a soft-deprecated alias for `conda-pypi-map = false`;
    // the deprecation warning is emitted when the manifest is parsed.
    if map.is_empty() {
        return Ok(PurlDerivationMode::Disabled);
    }

    let channel_to_entry_map = map
        .iter()
        .map(|(key, value)| {
            let key = key.clone().into_channel(channel_config).into_diagnostic()?;
            Ok((key, value))
        })
        .collect::<miette::Result<HashMap<Channel, &CondaPypiMapEntry>>>()?;

    validate_mapped_channels_are_used(manifest, channel_config, channel_to_entry_map.keys())?;

    let mapping = channel_to_entry_map
        .iter()
        .map(|(channel, entry)| {
            Ok((
                channel.canonical_name().trim_end_matches('/').into(),
                convert_entry(entry, channel_config)?,
            ))
        })
        .collect::<miette::Result<HashMap<ChannelName, ProjectDefinedChannelMapping>>>()?;

    Ok(PurlDerivationMode::ProjectDefined(
        ProjectDefinedMapping::new(mapping).into(),
    ))
}

/// Every channel in `conda-pypi-map` must appear in the workspace or feature
/// channels; an entry for an unused channel is almost certainly a typo.
fn validate_mapped_channels_are_used<'a>(
    manifest: &WorkspaceManifest,
    channel_config: &ChannelConfig,
    mapped_channels: impl Iterator<Item = &'a Channel>,
) -> miette::Result<()> {
    let project_channels: HashSet<_> = manifest
        .workspace
        .channels
        .iter()
        .map(|pc| pc.channel.clone().into_channel(channel_config))
        .try_collect()
        .into_diagnostic()?;

    let feature_channels: HashSet<_> = manifest
        .features
        .values()
        .flat_map(|feature| feature.channels.iter())
        .flatten()
        .map(|pc| pc.channel.clone().into_channel(channel_config))
        .try_collect()
        .into_diagnostic()?;

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
            miette::bail!(
                "conda-pypi-map is defined: the {} is missing from the channels array, which currently are: {}",
                console::style(
                    channel
                        .name
                        .clone()
                        .unwrap_or_else(|| channel.base_url.to_string())
                )
                .bold(),
                channels
            );
        }
    }
    Ok(())
}

/// Convert a manifest entry to the per-channel mapping configuration used by
/// the purl derivation client.
fn convert_entry(
    entry: &CondaPypiMapEntry,
    channel_config: &ChannelConfig,
) -> miette::Result<ProjectDefinedChannelMapping> {
    match entry {
        CondaPypiMapEntry::Disabled => Ok(ProjectDefinedChannelMapping::disabled()),
        CondaPypiMapEntry::Map(CondaPypiMapSpec {
            location,
            mapping,
            mode,
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
                        .map(|(name, pypi_name)| (name.to_lowercase(), pypi_name.clone()))
                        .collect(),
                ));
            }
            let mode = match mode {
                CondaPypiMapMode::Extend => MappingMode::Extend,
                CondaPypiMapMode::Replace => MappingMode::Replace,
            };
            Ok(ProjectDefinedChannelMapping::new(sources, mode))
        }
    }
}

/// Classify a manifest location spec into a url or a path, resolving relative
/// paths against the workspace root. `file://` urls are normalized to paths.
fn parse_mapping_location(
    spec: &MappingLocationSpec,
    channel_config: &ChannelConfig,
) -> miette::Result<ProjectDefinedMappingLocation> {
    let url_or_path = UrlOrPath::from_str(&spec.location)
        .into_diagnostic()
        .context(format!(
            "Could not parse mapping location `{}`",
            spec.location
        ))?;

    match url_or_path {
        UrlOrPath::Url(url) => {
            if !matches!(url.scheme(), "http" | "https") {
                miette::bail!(
                    "unsupported scheme `{}` in mapping location `{}`; only http(s) URLs and local paths are supported",
                    url.scheme(),
                    spec.location
                );
            }
            Ok(ProjectDefinedMappingLocation::Url {
                url,
                cache_ttl: spec.cache_ttl,
            })
        }
        UrlOrPath::Path(path) => {
            if spec.cache_ttl.is_some() {
                miette::bail!(
                    "`cache-ttl` is only supported for http(s) mapping locations, but `{}` is a local file",
                    spec.location
                );
            }
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
