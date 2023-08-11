use crate::{progress, project::Project};
use indicatif::ProgressBar;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Channel, Platform};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::{fetch, sparse::SparseRepoData};
use std::{path::Path, time::Duration};

impl Project {
    pub async fn fetch_sparse_repodata(&self) -> miette::Result<Vec<SparseRepoData>> {
        let channels = self.channels();
        let platforms = self.platforms();
        fetch_sparse_repodata(channels, platforms).await
    }
}

pub async fn fetch_sparse_repodata(
    channels: &[Channel],
    target_platforms: &[Platform],
) -> miette::Result<Vec<SparseRepoData>> {
    // Determine all the repodata that requires fetching.
    let mut fetch_targets = Vec::with_capacity(channels.len() * target_platforms.len());
    for channel in channels {
        // Determine the platforms to use for this channel.
        let platforms = channel.platforms.as_deref().unwrap_or(target_platforms);
        for platform in platforms {
            fetch_targets.push((channel.clone(), *platform));
        }

        // Add noarch if the channel did not specify explicit platforms.
        let noarch_missing = !platforms.contains(&Platform::NoArch) && channel.platforms.is_none();
        if noarch_missing {
            fetch_targets.push((channel.clone(), Platform::NoArch));
        }
    }

    // Construct a top-level progress bar
    let multi_progress = progress::global_multi_progress();
    let top_level_progress = multi_progress.add(ProgressBar::new(fetch_targets.len() as u64));
    top_level_progress.set_style(progress::long_running_progress_style());
    top_level_progress.set_message("fetching latest repodata");
    top_level_progress.enable_steady_tick(Duration::from_millis(50));

    let repodata_cache_path = rattler::default_cache_dir()
        .map_err(|_| miette::miette!("could not determine default cache directory"))?
        .join("repodata");
    let repodata_download_client = AuthenticatedClient::default();
    let multi_progress = progress::global_multi_progress();
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<miette::Result<Option<SparseRepoData>>>(fetch_targets.len());
    let mut progress_bars = Vec::new();
    for (channel, platform) in fetch_targets {
        // Construct a progress bar for the fetch
        let progress_bar = multi_progress.add(
            indicatif::ProgressBar::new(1)
                .with_prefix(format!("{}/{platform}", friendly_channel_name(&channel)))
                .with_style(progress::default_bytes_style()),
        );
        progress_bar.enable_steady_tick(Duration::from_millis(50));
        progress_bars.push(progress_bar.clone());

        // Spawn a future that downloads the repodata in the background
        let repodata_cache = repodata_cache_path.clone();
        let download_client = repodata_download_client.clone();
        let result_tx = tx.clone();
        tokio::spawn(async move {
            let fetch_result = fetch_repo_data_records_with_progress(
                channel,
                platform,
                &repodata_cache,
                download_client,
                progress_bar.clone(),
                platform != Platform::NoArch,
            )
            .await;

            // Silently ignore send error, it means the receiving end has been dropped and this
            // task was probably cancelled.
            let _ = result_tx.send(fetch_result).await;
        });
    }

    // No longer need the sending end of the results channel
    drop(tx);

    // Await all the results (including the failures)
    let mut result = Vec::new();
    let mut error = None;
    while let Some(fetch_result) = rx.recv().await {
        match fetch_result {
            Err(e) => {
                error = error.or(Some(e));
            }
            Ok(Some(data)) => result.push(data),
            Ok(None) => {}
        }
        top_level_progress.tick();
    }

    // Clear all the progressbars together
    for pb in progress_bars {
        pb.finish_and_clear()
    }

    // If there was an error, report it.
    if let Some(error) = error {
        return Err(error).wrap_err("failed to fetch repodata from channels");
    }

    Ok(result)
}

/// Given a channel and platform, download and cache the `repodata.json` for it. This function
/// reports its progress via a CLI progressbar.
async fn fetch_repo_data_records_with_progress(
    channel: Channel,
    platform: Platform,
    repodata_cache: &Path,
    client: AuthenticatedClient,
    progress_bar: indicatif::ProgressBar,
    allow_not_found: bool,
) -> miette::Result<Option<SparseRepoData>> {
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
            if matches!(&e, fetch::FetchRepoDataError::NotFound(_)) && allow_not_found {
                progress_bar.set_style(progress::finished_progress_style());
                progress_bar.finish_with_message("Not Found");
                return Ok(None);
            }

            progress_bar.set_style(progress::errored_progress_style());
            progress_bar.finish_with_message("404 not found");
            return Err(e).into_diagnostic();
        }
        Ok(result) => result,
    };

    // Notify that we are deserializing
    progress_bar.set_style(progress::deserializing_progress_style());
    progress_bar.set_message("Deserializing..");

    // Deserialize the data. This is a hefty blocking operation so we spawn it as a tokio blocking
    // task.
    let repo_data_json_path = result.repo_data_json_path.clone();
    match tokio::task::spawn_blocking(move || {
        SparseRepoData::new(channel, platform.to_string(), repo_data_json_path, None)
    })
    .await
    {
        Ok(Ok(repodata)) => {
            progress_bar.set_style(progress::finished_progress_style());
            let is_cache_hit = matches!(
                result.cache_result,
                fetch::CacheResult::CacheHit | fetch::CacheResult::CacheHitAfterFetch
            );
            progress_bar.finish_with_message(if is_cache_hit { "Using cache" } else { "Done" });
            Ok(Some(repodata))
        }
        Ok(Err(err)) => {
            progress_bar.set_style(progress::errored_progress_style());
            progress_bar.finish_with_message("Error");
            Err(err).into_diagnostic()
        }
        Err(err) => match err.try_into_panic() {
            Ok(panic) => {
                std::panic::resume_unwind(panic);
            }
            Err(_) => {
                progress_bar.set_style(progress::errored_progress_style());
                progress_bar.finish_with_message("Cancelled..");
                // Since the task was cancelled most likely the whole async stack is being cancelled.
                Err(miette::miette!("cancelled"))
            }
        },
    }
}

/// Returns a friendly name for the specified channel.
pub fn friendly_channel_name(channel: &Channel) -> String {
    channel
        .name
        .as_ref()
        .map(String::from)
        .unwrap_or_else(|| channel.canonical_name())
}
