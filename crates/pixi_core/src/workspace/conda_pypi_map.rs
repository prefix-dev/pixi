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

    // The manifest map can spell the same channel in different forms (by
    // name and by URL) that only collapse once resolved to a `Channel`.
    // Collecting blindly would keep a nondeterministic winner, so reject
    // duplicates instead.
    let mut channel_to_entry_map: HashMap<Channel, &CondaPypiMapEntry> =
        HashMap::with_capacity(map.len());
    for (key, value) in map {
        let channel = key.clone().into_channel(channel_config).into_diagnostic()?;
        let channel_name = channel
            .name
            .clone()
            .unwrap_or_else(|| channel.base_url.to_string());
        if channel_to_entry_map.insert(channel, value).is_some() {
            miette::bail!(
                "the channel {} is configured more than once in `conda-pypi-map` \
                 (e.g. both by name and by URL); keep a single entry per channel",
                console::style(channel_name).bold(),
            );
        }
    }

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
            Ok(ProjectDefinedChannelMapping::new(
                sources,
                convert_mode(*mode),
            ))
        }
    }
}

/// Convert the manifest-level mode to the derivation-level [`MappingMode`].
///
/// The two enums are deliberately asymmetric: `MappingMode::Disabled` has no
/// manifest-level mode string because "disabled" is spelled `<channel> =
/// false` in TOML (see [`CondaPypiMapEntry::Disabled`]), not `mode =
/// "disabled"`. This function and the `Disabled` arm in [`convert_entry`] are
/// the single place where the two representations meet. (A `From` impl cannot
/// encode this: neither `pixi_manifest` nor `pypi_mapping` depends on the
/// other, so the orphan rule forces the conversion to live here.)
fn convert_mode(mode: CondaPypiMapMode) -> MappingMode {
    match mode {
        CondaPypiMapMode::Extend => MappingMode::Extend,
        CondaPypiMapMode::Replace => MappingMode::Replace,
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
            // A plaintext mapping URL can be tampered with on the network,
            // and a tampered mapping changes which conda packages are
            // considered to satisfy PyPI dependencies.
            if url.scheme() == "http" {
                tracing::warn!(
                    "the conda-pypi mapping location `{}` uses plain `http://`; the mapping can \
                     be tampered with in transit. Prefer `https://` or a local file.",
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

#[cfg(test)]
mod test {
    use std::time::Duration;

    use super::*;

    fn channel_config() -> ChannelConfig {
        ChannelConfig::default_with_root_dir(PathBuf::from("/workspace"))
    }

    fn location(location: &str, cache_ttl: Option<Duration>) -> MappingLocationSpec {
        MappingLocationSpec {
            location: location.to_string(),
            cache_ttl,
        }
    }

    #[test]
    fn test_parse_mapping_location_http_url_with_ttl() {
        let ttl = Some(Duration::from_secs(60));
        let parsed = parse_mapping_location(
            &location("https://example.com/m.json", ttl),
            &channel_config(),
        )
        .unwrap();
        assert_eq!(
            parsed,
            ProjectDefinedMappingLocation::Url {
                url: "https://example.com/m.json".parse().unwrap(),
                cache_ttl: ttl,
            }
        );
    }

    #[test]
    fn test_parse_mapping_location_relative_path() {
        let parsed =
            parse_mapping_location(&location("sub/m.json", None), &channel_config()).unwrap();
        assert_eq!(
            parsed,
            ProjectDefinedMappingLocation::Path(PathBuf::from("/workspace/sub/m.json"))
        );
    }

    #[test]
    fn test_parse_mapping_location_file_url_becomes_path() {
        let parsed =
            parse_mapping_location(&location("file:///abs/m.json", None), &channel_config())
                .unwrap();
        assert_eq!(
            parsed,
            ProjectDefinedMappingLocation::Path(PathBuf::from("/abs/m.json"))
        );
    }

    #[test]
    fn test_parse_mapping_location_rejects_ttl_on_path() {
        let err = parse_mapping_location(
            &location("sub/m.json", Some(Duration::from_secs(60))),
            &channel_config(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("cache-ttl"));
    }

    #[test]
    fn test_parse_mapping_location_rejects_unsupported_scheme() {
        let err = parse_mapping_location(
            &location("ftp://example.com/m.json", None),
            &channel_config(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("unsupported scheme"));
    }

    #[test]
    fn test_convert_entry_disabled() {
        let converted = convert_entry(&CondaPypiMapEntry::Disabled, &channel_config()).unwrap();
        assert_eq!(converted, ProjectDefinedChannelMapping::disabled());
    }
}
