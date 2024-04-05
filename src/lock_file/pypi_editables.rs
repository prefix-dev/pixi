use std::{path::Path, str::FromStr, sync::Arc};

use distribution_filename::WheelFilename;
use distribution_types::LocalEditable;
use futures::StreamExt;
use install_wheel_rs::metadata::read_archive_metadata;
use itertools::Itertools;
use pypi_types::Metadata23;
use requirements_txt::EditableRequirement;
use uv_cache::Cache;
use uv_dispatch::BuildDispatch;
use uv_installer::DownloadReporter;
use uv_traits::{BuildContext, BuildKind, SourceBuildTrait};
use zip::ZipArchive;

use crate::uv_reporter::{UvReporter, UvReporterOptions};

#[derive(thiserror::Error, Debug)]
pub enum BuildEditablesError {
    #[error("error during setting up of editable build environment")]
    BuildSetup {
        // Because of anyhow error
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("error during editable metadata extraction")]
    Metadata {
        // Because of anyhow error
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("error during editable build")]
    Build {
        #[from]
        source: uv_build::Error,
    },

    #[error("error during parsing of editable metadata")]
    MetadataParse {
        #[from]
        source: pypi_types::Error,
    },

    #[error("error creating temporary dir for editable wheel build")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("error reading wheel metadata for editable")]
    WheelMetadata {
        #[from]
        source: install_wheel_rs::Error,
    },

    #[error("error open wheel archive for editable built wheel")]
    Zip {
        #[from]
        source: zip::result::ZipError,
    },

    #[error("error parsing wheel filename for editable")]
    WheelFilename {
        #[from]
        source: distribution_filename::WheelFilenameError,
    },
}

/// Read the [`Metadata23`] from a built wheel.
fn read_wheel_metadata(
    filename: &WheelFilename,
    wheel: &Path,
) -> Result<Metadata23, BuildEditablesError> {
    let file = std::fs::File::open(wheel)?;
    let reader = std::io::BufReader::new(file);
    let mut archive = ZipArchive::new(reader)?;
    let dist_info = read_archive_metadata(filename, &mut archive)?;
    Ok(Metadata23::parse_metadata(&dist_info)?)
}

async fn build_editable(
    cache: &Cache,
    local_editable: &LocalEditable,
    build_dispatch: &BuildDispatch<'_>,
) -> Result<Metadata23, BuildEditablesError> {
    let mut source_build = build_dispatch
        .setup_build(
            &local_editable.path,
            None,
            &local_editable.to_string(),
            None,
            BuildKind::Editable,
        )
        .await
        .map_err(|err| BuildEditablesError::BuildSetup { source: err.into() })?;

    let disk_filename = source_build
        .metadata()
        .await
        .map_err(|err| BuildEditablesError::Metadata { source: err.into() })?;

    // Check if we have the metadata director
    // if this is None we have no option to build the wheel
    if let Some(metadata_directory) = disk_filename {
        let content = std::fs::read(metadata_directory.join("METADATA"))
            .map_err(|e| BuildEditablesError::Metadata { source: e.into() })?;
        Ok(Metadata23::parse_metadata(&content)?)
    } else {
        let temp_dir = tempfile::tempdir_in(cache.root())?;
        let wheel = source_build
            .build(temp_dir.path())
            .await
            .map_err(|e| BuildEditablesError::Build { source: e })?;
        Ok(read_wheel_metadata(
            &WheelFilename::from_str(&wheel)?,
            &temp_dir.path().join(wheel),
        )?)
    }
}

/// Build a set of editable distributions.
#[allow(clippy::too_many_arguments)]
pub async fn build_editables(
    editables: &[EditableRequirement],
    cache: &Cache,
    build_dispatch: &BuildDispatch<'_>,
) -> Result<Vec<(LocalEditable, Metadata23)>, BuildEditablesError> {
    let options = UvReporterOptions::new()
        .with_length(editables.len() as u64)
        .with_capacity(editables.len() + 30)
        .with_starting_tasks(editables.iter().map(|d| format!("{}", d.path.display())))
        .with_top_level_message("Building editables");

    let editables: Vec<LocalEditable> = editables
        .iter()
        .map(|editable| {
            let EditableRequirement { url, extras, path } = editable;
            LocalEditable {
                url: url.clone(),
                extras: extras.clone(),
                path: path.clone(),
            }
        })
        .collect_vec();

    let reporter = Arc::new(UvReporter::new(options));
    let mut editables_and_metadata = Vec::new();

    let reporter_clone = reporter.clone();
    let mut build_stream = futures::stream::iter(editables)
        .map(|editable| async {
            let task = reporter_clone.on_editable_build_start(&editable);
            let metadata = build_editable(cache, &editable, build_dispatch).await?;
            Ok::<_, BuildEditablesError>((editable, metadata, task))
        })
        .buffer_unordered(10);

    let reporter_clone = reporter.clone();
    while let Some((local_editable, metadata, task)) = build_stream.next().await.transpose()? {
        reporter_clone.on_editable_build_complete(&local_editable, task);
        editables_and_metadata.push((local_editable, metadata));
    }

    Ok(editables_and_metadata)
}
