use std::path::PathBuf;

use distribution_types::{FlatIndexLocation, IndexLocations, IndexUrl};
use pep508_rs::{VerbatimUrl, VerbatimUrlError};
use pixi_manifest::pypi::pypi_options::PypiOptions;
use pixi_manifest::pypi::GitRev;
use rattler_lock::FindLinksUrlOrPath;
use uv_git::GitReference;

#[derive(thiserror::Error, Debug)]
pub enum ConvertFlatIndexLocationError {
    #[error("could not convert path to flat index location {1}")]
    VerbatimUrlError(#[source] VerbatimUrlError, PathBuf),
}

/// Converts to the [`distribution_types::FlatIndexLocation`]
pub fn to_flat_index_location(
    find_links: &FindLinksUrlOrPath,
) -> Result<FlatIndexLocation, ConvertFlatIndexLocationError> {
    match find_links {
        FindLinksUrlOrPath::Path(path) => Ok(FlatIndexLocation::Path(
            VerbatimUrl::from_path(path.clone())
                .map_err(|e| ConvertFlatIndexLocationError::VerbatimUrlError(e, path.clone()))?,
        )),
        FindLinksUrlOrPath::Url(url) => {
            Ok(FlatIndexLocation::Url(VerbatimUrl::from_url(url.clone())))
        }
    }
}

/// Convert the subset of pypi-options to index locations
pub fn pypi_options_to_index_locations(
    options: &PypiOptions,
) -> Result<IndexLocations, ConvertFlatIndexLocationError> {
    // Convert the index to a `IndexUrl`
    let index = options
        .index_url
        .clone()
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from);

    // Convert to list of extra indexes
    let extra_indexes = options
        .extra_index_urls
        .clone()
        .map(|urls| {
            urls.into_iter()
                .map(VerbatimUrl::from_url)
                .map(IndexUrl::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let flat_indexes = if let Some(flat_indexes) = options.find_links.clone() {
        // Convert to list of flat indexes
        let flat_indexes = flat_indexes
            .into_iter()
            .map(|i| to_flat_index_location(&i))
            .collect::<Result<Vec<_>, _>>()?;

        flat_indexes
    } else {
        vec![]
    };

    // we don't have support for an explicit `no_index` field in the `PypiOptions`
    // so we only set it if you want to use flat indexes only
    let no_index = index.is_none() && !flat_indexes.is_empty();
    Ok(IndexLocations::new(
        index,
        extra_indexes,
        flat_indexes,
        no_index,
    ))
}

/// Convert locked indexes to IndexLocations
pub fn locked_indexes_to_index_locations(
    indexes: &rattler_lock::PypiIndexes,
) -> Result<IndexLocations, ConvertFlatIndexLocationError> {
    let index = indexes
        .indexes
        .first()
        .cloned()
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from);
    let extra_indexes = indexes
        .indexes
        .iter()
        .skip(1)
        .cloned()
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from)
        .collect::<Vec<_>>();
    let flat_indexes = indexes
        .find_links
        .iter()
        .map(to_flat_index_location)
        .collect::<Result<Vec<_>, _>>()?;

    // we don't have support for an explicit `no_index` field in the `PypiIndexes`
    // so we only set it if you want to use flat indexes only
    let no_index = index.is_none() && !flat_indexes.is_empty();
    Ok(IndexLocations::new(
        index,
        extra_indexes,
        flat_indexes,
        no_index,
    ))
}

pub fn to_git_reference(rev: &GitRev) -> GitReference {
    match rev {
        GitRev::Full(rev) => GitReference::FullCommit(rev.clone()),
        GitRev::Short(rev) => GitReference::BranchOrTagOrCommit(rev.clone()),
    }
}
