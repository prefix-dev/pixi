use distribution_types::{FlatIndexLocation, IndexLocations, IndexUrl};
use pep508_rs::VerbatimUrl;
use pixi_manifest::pypi::pypi_options::PypiOptions;
use pixi_manifest::pypi::GitRev;
use rattler_lock::FindLinksUrlOrPath;
use uv_git::GitReference;

/// Converts to the [`distribution_types::FlatIndexLocation`]
pub fn to_flat_index_location(find_links: &FindLinksUrlOrPath) -> FlatIndexLocation {
    match find_links {
        FindLinksUrlOrPath::Path(path) => FlatIndexLocation::Path(path.clone()),
        FindLinksUrlOrPath::Url(url) => FlatIndexLocation::Url(url.clone()),
    }
}

/// Convert the subset of pypi-options to index locations
pub fn pypi_options_to_index_locations(options: &PypiOptions) -> IndexLocations {
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

    // Convert to list of flat indexes
    let flat_indexes = options
        .find_links
        .clone()
        .map(|indexes| {
            indexes
                .into_iter()
                .map(|index| to_flat_index_location(&index))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // We keep the `no_index` to false for now, because I've not seen a use case for it yet
    // we could change this later if needed
    IndexLocations::new(index, extra_indexes, flat_indexes, false)
}

/// Convert locked indexes to IndexLocations
pub fn locked_indexes_to_index_locations(indexes: &rattler_lock::PypiIndexes) -> IndexLocations {
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
        .collect::<Vec<_>>();
    IndexLocations::new(index, extra_indexes, flat_indexes, false)
}

pub fn to_git_reference(rev: &GitRev) -> GitReference {
    match rev {
        GitRev::Full(rev) => GitReference::FullCommit(rev.clone()),
        GitRev::Short(rev) => GitReference::BranchOrTagOrCommit(rev.clone()),
    }
}
