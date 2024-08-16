use futures::StreamExt;
use indicatif::ProgressBar;
use miette::IntoDiagnostic;
use rattler_conda_types::{Channel, Platform};
use rattler_repodata_gateway::fetch::FetchRepoDataOptions;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_repodata_gateway::{fetch, Reporter};
use reqwest_middleware::ClientWithMiddleware;
use std::path::Path;
use std::sync::Arc;
use url::Url;

struct DownloadProgressReporter {
    progress_bar: ProgressBar,
}

impl Reporter for DownloadProgressReporter {
    fn on_download_progress(&self, _url: &Url, _index: usize, bytes: usize, total: Option<usize>) {
        self.progress_bar.set_length(total.unwrap_or(bytes) as u64);
        self.progress_bar.set_position(bytes as u64);
    }
}

/// Given a channel and platform, download and cache the `repodata.json` for it. This function
/// reports its progress via a CLI progressbar.
async fn fetch_repo_data_records_with_progress(
    channel: Channel,
    platform: Platform,
    repodata_cache: &Path,
    client: ClientWithMiddleware,
    progress_bar: indicatif::ProgressBar,
    allow_not_found: bool,
    fetch_options: FetchRepoDataOptions,
) -> miette::Result<Option<SparseRepoData>> {
    // Download the repodata.json
    let result = fetch::fetch_repo_data(
        channel.platform_url(platform),
        client,
        repodata_cache.to_path_buf(),
        fetch_options,
        Some(Arc::new(DownloadProgressReporter {
            progress_bar: progress_bar.clone(),
        })),
    )
    .await;

    // Error out if an error occurred, but also update the progress bar
    let result = match result {
        Err(e) => {
            if matches!(&e, fetch::FetchRepoDataError::NotFound(_)) && allow_not_found {
                progress_bar.set_style(pixi_progress::finished_progress_style());
                progress_bar.finish_with_message("Not Found");
                return Ok(None);
            }

            progress_bar.set_style(pixi_progress::errored_progress_style());
            progress_bar.finish_with_message("404 not found");
            return Err(e).into_diagnostic();
        }
        Ok(result) => result,
    };

    // Notify that we are deserializing
    progress_bar.set_style(pixi_progress::deserializing_progress_style());
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
            progress_bar.set_style(pixi_progress::finished_progress_style());
            let is_cache_hit = matches!(
                result.cache_result,
                fetch::CacheResult::CacheHit | fetch::CacheResult::CacheHitAfterFetch
            );
            progress_bar.finish_with_message(if is_cache_hit { "Using cache" } else { "Done" });
            Ok(Some(repodata))
        }
        Ok(Err(err)) => {
            progress_bar.set_style(pixi_progress::errored_progress_style());
            progress_bar.finish_with_message("Error");
            Err(err).into_diagnostic()
        }
        Err(err) => match err.try_into_panic() {
            Ok(panic) => {
                std::panic::resume_unwind(panic);
            }
            Err(_) => {
                progress_bar.set_style(pixi_progress::errored_progress_style());
                progress_bar.finish_with_message("Cancelled..");
                // Since the task was cancelled most likely the whole async stack is being cancelled.
                Err(miette::miette!("cancelled"))
            }
        },
    }
}

/// Returns a friendly name for the specified channel.
pub(crate) fn friendly_channel_name(channel: &Channel) -> String {
    channel
        .name
        .as_ref()
        .map(String::from)
        .unwrap_or_else(|| channel.canonical_name())
}
