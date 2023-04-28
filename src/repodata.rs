use crate::progress;
use crate::progress::{
    default_bytes_style, deserializing_progress_style, errored_progress_style,
    finished_progress_style,
};
use crate::project::Project;
use futures::StreamExt;
use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_repodata_gateway::{fetch, sparse::SparseRepoData};
use reqwest::{Client, StatusCode};
use std::{path::Path, time::Duration};

impl Project {
    pub async fn fetch_sparse_repodata(&self) -> anyhow::Result<Vec<SparseRepoData>> {
        let channels = self.channels(&ChannelConfig::default())?;
        let target_platforms = self.platforms()?;

        // Determine all the repodata that requires fetching.
        let mut fetch_targets = Vec::with_capacity(channels.len() * target_platforms.len());
        for channel in channels {
            // Determine the platforms to use for this channel.
            let platforms = match &channel.platforms {
                None => &target_platforms[..],
                Some(platforms) => &platforms[..],
            };

            for platform in platforms {
                fetch_targets.push((channel.clone(), *platform));
            }

            // Add noarch if the channel did not specify explicit platforms.
            let noarch_missing =
                !platforms.contains(&Platform::NoArch) && channel.platforms.is_none();
            if noarch_missing {
                fetch_targets.push((channel.clone(), Platform::NoArch));
            }
        }

        // Start fetching all repodata
        let channel_and_platform_len = fetch_targets.len();
        let repodata_cache_path = rattler::default_cache_dir()?.join("repodata");
        let repodata_download_client = Client::default();
        let multi_progress = progress::global_multi_progress();
        let sparse_repo_datas = futures::stream::iter(fetch_targets)
            .map(move |(channel, platform)| {
                let repodata_cache = repodata_cache_path.clone();
                let download_client = repodata_download_client.clone();
                let multi_progress = multi_progress.clone();
                async move {
                    fetch_repo_data_records_with_progress(
                        channel,
                        platform,
                        &repodata_cache,
                        download_client.clone(),
                        multi_progress,
                        platform == Platform::NoArch,
                    )
                    .await
                }
            })
            .buffer_unordered(channel_and_platform_len)
            .filter_map(|result| async move {
                match result {
                    Err(e) => Some(Err(e)),
                    Ok(Some(data)) => Some(Ok(data)),
                    Ok(None) => None,
                }
            })
            .collect::<Vec<_>>()
            .await
            // Collect into another iterator where we extract the first erroneous result
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;

        Ok(sparse_repo_datas)
    }
}

/// Given a channel and platform, download and cache the `repodata.json` for it. This function
/// reports its progress via a CLI progressbar.
async fn fetch_repo_data_records_with_progress(
    channel: Channel,
    platform: Platform,
    repodata_cache: &Path,
    client: Client,
    multi_progress: indicatif::MultiProgress,
    allow_not_found: bool,
) -> Result<Option<SparseRepoData>, anyhow::Error> {
    // Create a progress bar
    let progress_bar = multi_progress.add(
        indicatif::ProgressBar::new(1)
            .with_finish(indicatif::ProgressFinish::AndLeave)
            .with_prefix(format!("{}/{platform}", friendly_channel_name(&channel)))
            .with_style(default_bytes_style()),
    );
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    // Download the repodata.json
    let download_progress_progress_bar = progress_bar.clone();
    let result = fetch::fetch_repo_data(
        channel.platform_url(platform),
        client,
        repodata_cache,
        fetch::FetchRepoDataOptions {
            download_progress: Some(Box::new(move |fetch::DownloadProgress { total, bytes }| {
                download_progress_progress_bar.set_length(total.unwrap_or(bytes));
                download_progress_progress_bar.set_position(bytes);
            })),
            ..Default::default()
        },
    )
    .await;

    // Error out if an error occurred, but also update the progress bar
    let result = match result {
        Err(e) => {
            let not_found = matches!(&e,
                fetch::FetchRepoDataError::HttpError(e) if e.status() == Some(StatusCode::NOT_FOUND)
            );
            if not_found && allow_not_found {
                progress_bar.set_style(finished_progress_style());
                progress_bar.finish_with_message("Not Found");
                return Ok(None);
            }

            progress_bar.set_style(errored_progress_style());
            progress_bar.finish_with_message("Error");
            return Err(e.into());
        }
        Ok(result) => result,
    };

    // Notify that we are deserializing
    progress_bar.set_style(deserializing_progress_style());
    progress_bar.set_message("Deserializing..");

    // Deserialize the data. This is a hefty blocking operation so we spawn it as a tokio blocking
    // task.
    let repo_data_json_path = result.repo_data_json_path.clone();
    match tokio::task::spawn_blocking(move || {
        SparseRepoData::new(channel, platform.to_string(), repo_data_json_path)
    })
    .await
    {
        Ok(Ok(repodata)) => {
            progress_bar.set_style(finished_progress_style());
            let is_cache_hit = matches!(
                result.cache_result,
                fetch::CacheResult::CacheHit | fetch::CacheResult::CacheHitAfterFetch
            );
            progress_bar.finish_with_message(if is_cache_hit { "Using cache" } else { "Done" });
            Ok(Some(repodata))
        }
        Ok(Err(err)) => {
            progress_bar.set_style(errored_progress_style());
            progress_bar.finish_with_message("Error");
            Err(err.into())
        }
        Err(err) => match err.try_into_panic() {
            Ok(panic) => {
                std::panic::resume_unwind(panic);
            }
            Err(_) => {
                progress_bar.set_style(errored_progress_style());
                progress_bar.finish_with_message("Cancelled..");
                // Since the task was cancelled most likely the whole async stack is being cancelled.
                Err(anyhow::anyhow!("cancelled"))
            }
        },
    }
}

/// Returns a friendly name for the specified channel.
fn friendly_channel_name(channel: &Channel) -> String {
    channel
        .name
        .as_ref()
        .map(String::from)
        .unwrap_or_else(|| channel.canonical_name())
}
