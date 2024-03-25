use std::path::Path;

use distribution_types::LocalEditable;
use platform_tags::Tags;
use requirements_txt::EditableRequirement;
use uv_cache::Cache;
use uv_client::RegistryClient;
use uv_dispatch::BuildDispatch;
use uv_installer::{BuiltEditable, Downloader};

use crate::uv_reporter::{UvReporter, UvReporterOptions};

/// TODO: remove this type when the uv error is exposed in the public API.
#[derive(thiserror::Error, Debug)]
pub enum BuildEditablesError {
    #[error("Failed to build editables: {source}")]
    DownloadError { source: Box<dyn std::error::Error> },
}

/// Build a set of editable distributions.
#[allow(clippy::too_many_arguments)]
pub async fn build_editables(
    editables: &[EditableRequirement],
    editable_wheel_dir: &Path,
    cache: &Cache,
    tags: &Tags,
    client: &RegistryClient,
    build_dispatch: &BuildDispatch<'_>,
) -> Result<Vec<BuiltEditable>, BuildEditablesError> {
    let options = UvReporterOptions::new()
        .with_length(editables.len() as u64)
        .with_capacity(editables.len() + 30)
        .with_starting_tasks(editables.iter().map(|d| format!("{}", d.path.display())))
        .with_top_level_message("Building editables");

    let downloader = Downloader::new(cache, tags, client, build_dispatch)
        .with_reporter(UvReporter::new(options));

    let editables: Vec<LocalEditable> = editables
        .iter()
        .map(|editable| {
            let EditableRequirement { url, extras, path } = editable;
            Ok(LocalEditable {
                url: url.clone(),
                extras: extras.clone(),
                path: path.clone(),
            })
        })
        .collect::<Result<_, _>>()?;

    let editables: Vec<_> = downloader
        .build_editables(editables, editable_wheel_dir)
        .await
        .map_err(|e| BuildEditablesError::DownloadError {
            source: Box::new(e),
        })?
        .into_iter()
        .collect();

    Ok(editables)
}
