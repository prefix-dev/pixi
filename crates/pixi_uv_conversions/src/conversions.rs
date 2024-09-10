use std::path::{Path, PathBuf};

use distribution_types::{FlatIndexLocation, IndexLocations, IndexUrl};
use pep508_rs::{
    InvalidNameError, PackageName, UnnamedRequirementUrl, VerbatimUrl, VerbatimUrlError,
};
use pixi_manifest::pypi::{
    pypi_options::{IndexStrategy, PypiOptions},
    GitRev,
};
use rattler_lock::FindLinksUrlOrPath;
use uv_git::GitReference;
use uv_python::PythonEnvironment;

#[derive(thiserror::Error, Debug)]
pub enum ConvertFlatIndexLocationError {
    #[error("could not convert path to flat index location {1}")]
    VerbatimUrlError(#[source] VerbatimUrlError, PathBuf),
}

/// Converts to the [`distribution_types::FlatIndexLocation`]
pub fn to_flat_index_location(
    find_links: &FindLinksUrlOrPath,
    base_path: &Path,
) -> Result<FlatIndexLocation, ConvertFlatIndexLocationError> {
    match find_links {
        FindLinksUrlOrPath::Path(path) => Ok(FlatIndexLocation::Path(
            VerbatimUrl::parse_path(path.clone(), base_path)
                .map_err(|e| ConvertFlatIndexLocationError::VerbatimUrlError(e, path.clone()))?
                .with_given(path.display().to_string()),
        )),
        FindLinksUrlOrPath::Url(url) => {
            Ok(FlatIndexLocation::Url(VerbatimUrl::from_url(url.clone())))
        }
    }
}

/// Convert the subset of pypi-options to index locations
pub fn pypi_options_to_index_locations(
    options: &PypiOptions,
    base_path: &Path,
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
        flat_indexes
            .into_iter()
            .map(|i| to_flat_index_location(&i, base_path))
            .collect::<Result<Vec<_>, _>>()?
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
    base_path: &Path,
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
        .map(|find_link| to_flat_index_location(find_link, base_path))
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

fn packages_to_build_isolation<'a>(
    names: Option<&'a [PackageName]>,
    python_environment: &'a PythonEnvironment,
) -> uv_types::BuildIsolation<'a> {
    return if let Some(package_names) = names {
        uv_types::BuildIsolation::SharedPackage(python_environment, package_names)
    } else {
        uv_types::BuildIsolation::default()
    };
}

/// Convert optional list of strings to package names
pub fn isolated_names_to_packages(
    names: Option<&[String]>,
) -> Result<Option<Vec<PackageName>>, InvalidNameError> {
    if let Some(names) = names {
        let names = names
            .iter()
            .map(|n| n.parse())
            .collect::<Result<Vec<PackageName>, _>>()?;
        Ok(Some(names))
    } else {
        Ok(None)
    }
}

/// Convert optional list of package names to build isolation
pub fn names_to_build_isolation<'a>(
    names: Option<&'a [PackageName]>,
    env: &'a PythonEnvironment,
) -> uv_types::BuildIsolation<'a> {
    packages_to_build_isolation(names, env)
}

/// Convert pixi `IndexStrategy` to `uv_types::IndexStrategy`
pub fn to_index_strategy(
    index_strategy: Option<&IndexStrategy>,
) -> uv_configuration::IndexStrategy {
    if let Some(index_strategy) = index_strategy {
        match index_strategy {
            IndexStrategy::FirstIndex => uv_configuration::IndexStrategy::FirstIndex,
            IndexStrategy::UnsafeFirstMatch => uv_configuration::IndexStrategy::UnsafeFirstMatch,
            IndexStrategy::UnsafeBestMatch => uv_configuration::IndexStrategy::UnsafeBestMatch,
        }
    } else {
        uv_configuration::IndexStrategy::default()
    }
}
