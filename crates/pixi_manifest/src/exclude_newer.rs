use chrono::{DateTime, Utc};
use pixi_spec::{ExcludeNewer, ResolvedExcludeNewer};
use rattler_conda_types::{ChannelUrl, PackageName, ParseChannelError};

use crate::PrioritizedChannel;

/// Combines a base `exclude-newer` cutoff with channel and package overrides
/// into absolute cutoffs for the conda solver.
///
/// Without a base cutoff, packages are only excluded when one of the
/// overrides applies to them.
pub fn resolve_exclude_newer<'a, F>(
    base: Option<ExcludeNewer>,
    channels: impl IntoIterator<Item = &'a PrioritizedChannel>,
    mut channel_key: F,
    package_overrides: impl IntoIterator<Item = (&'a PackageName, &'a ExcludeNewer)>,
) -> Result<Option<ResolvedExcludeNewer>, ParseChannelError>
where
    F: FnMut(&PrioritizedChannel) -> Result<ChannelUrl, ParseChannelError>,
{
    let mut exclude_newer = base.map(|config| ResolvedExcludeNewer::from_datetime(config.cutoff()));

    for channel in channels {
        let Some(channel_exclude_newer) = channel.exclude_newer else {
            continue;
        };

        let channel_key = channel_key(channel)?;
        let config = exclude_newer
            .get_or_insert_with(|| ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC));
        *config = config
            .clone()
            .with_channel_cutoff(channel_key, channel_exclude_newer.cutoff());
    }

    for (name, package_exclude_newer) in package_overrides {
        let config = exclude_newer
            .get_or_insert_with(|| ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC));
        *config = config
            .clone()
            .with_package_cutoff(name.clone(), package_exclude_newer.cutoff());
    }

    Ok(exclude_newer)
}
