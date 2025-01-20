use std::path::{Path, PathBuf};
use std::str::FromStr;

use pixi_git::sha::GitSha;
use pixi_manifest::pypi::pypi_options::FindLinksUrlOrPath;
use pixi_manifest::pypi::pypi_options::{IndexStrategy, PypiOptions};
use pixi_record::{PinnedGitCheckout, PinnedGitSpec};
use pixi_spec::Reference as PixiReference;

use pixi_git::git::GitReference as PixiGitReference;

use uv_distribution_types::{GitSourceDist, Index, IndexLocations, IndexUrl};
use uv_git::GitReference;
use uv_pep508::{InvalidNameError, PackageName, VerbatimUrl, VerbatimUrlError};
use uv_python::PythonEnvironment;

#[derive(thiserror::Error, Debug)]
pub enum ConvertFlatIndexLocationError {
    #[error("could not convert path to flat index location {1}")]
    VerbatimUrlError(#[source] VerbatimUrlError, PathBuf),
    #[error("base path is not absolute: {path}", path = .0.display())]
    NotAbsolute(PathBuf),
}

/// Convert the subset of pypi-options to index locations
pub fn pypi_options_to_index_locations(
    options: &PypiOptions,
    base_path: &Path,
) -> Result<IndexLocations, ConvertFlatIndexLocationError> {
    // Check if the base path is absolute
    // Otherwise uv might panic
    if !base_path.is_absolute() {
        return Err(ConvertFlatIndexLocationError::NotAbsolute(
            base_path.to_path_buf(),
        ));
    }

    // Convert the index to a `IndexUrl`
    let index = options
        .index_url
        .clone()
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from)
        .map(Index::from_index_url)
        .into_iter();

    // Convert to list of extra indexes
    let extra_indexes = options
        .extra_index_urls
        .clone()
        .into_iter()
        .flat_map(|urls| {
            urls.into_iter()
                .map(VerbatimUrl::from_url)
                .map(IndexUrl::from)
                .map(Index::from_extra_index_url)
        });

    let flat_indexes = if let Some(flat_indexes) = options.find_links.clone() {
        // Convert to list of flat indexes
        flat_indexes
            .into_iter()
            .map(|url| match url {
                FindLinksUrlOrPath::Path(relative) => VerbatimUrl::from_path(&relative, base_path)
                    .map_err(|e| ConvertFlatIndexLocationError::VerbatimUrlError(e, relative)),
                FindLinksUrlOrPath::Url(url) => Ok(VerbatimUrl::from_url(url.clone())),
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(IndexUrl::from)
            .map(Index::from_find_links)
            .collect()
    } else {
        vec![]
    };

    // we don't have support for an explicit `no_index` field in the `PypiIndexes`
    // so we only set it if you want to use flat indexes only
    let indexes: Vec<_> = index.chain(extra_indexes).collect();
    let no_index = indexes.is_empty() && !flat_indexes.is_empty();

    Ok(IndexLocations::new(indexes, flat_indexes, no_index))
}

/// Convert locked indexes to IndexLocations
pub fn locked_indexes_to_index_locations(
    indexes: &rattler_lock::PypiIndexes,
    base_path: &Path,
) -> Result<IndexLocations, ConvertFlatIndexLocationError> {
    // Check if the base path is absolute
    // Otherwise uv might panic
    if !base_path.is_absolute() {
        return Err(ConvertFlatIndexLocationError::NotAbsolute(
            base_path.to_path_buf(),
        ));
    }

    let index = indexes
        .indexes
        .first()
        .cloned()
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from)
        .map(Index::from_index_url)
        .into_iter();
    let extra_indexes = indexes
        .indexes
        .iter()
        .skip(1)
        .cloned()
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from)
        .map(Index::from_extra_index_url);
    let flat_indexes = indexes
        .find_links
        .iter()
        .map(|url| match url {
            rattler_lock::FindLinksUrlOrPath::Path(relative) => {
                VerbatimUrl::from_path(relative, base_path).map_err(|e| {
                    ConvertFlatIndexLocationError::VerbatimUrlError(e, relative.clone())
                })
            }
            rattler_lock::FindLinksUrlOrPath::Url(url) => Ok(VerbatimUrl::from_url(url.clone())),
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(IndexUrl::from)
        .map(Index::from_find_links)
        .collect();

    // we don't have support for an explicit `no_index` field in the `PypiIndexes`
    // so we only set it if you want to use flat indexes only
    let indexes: Vec<_> = index.chain(extra_indexes).collect();
    let flat_index: Vec<_> = flat_indexes;
    let no_index = indexes.is_empty() && !flat_index.is_empty();
    Ok(IndexLocations::new(indexes, flat_index, no_index))
}

// pub fn to_git_reference(rev: &GitRev) -> GitReference {
//     match rev {
//         GitRev::Full(rev) => GitReference::FullCommit(rev.clone()),
//         GitRev::Short(rev) => GitReference::BranchOrTagOrCommit(rev.clone()),
//     }
// }

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

pub fn into_uv_git_reference(git_ref: PixiGitReference) -> GitReference {
    match git_ref {
        PixiGitReference::Branch(branch) => GitReference::Branch(branch),
        PixiGitReference::Tag(tag) => GitReference::Tag(tag),
        PixiGitReference::ShortCommit(rev) => GitReference::ShortCommit(rev),
        PixiGitReference::BranchOrTag(rev) => GitReference::BranchOrTag(rev),
        PixiGitReference::BranchOrTagOrCommit(rev) => GitReference::BranchOrTagOrCommit(rev),
        PixiGitReference::NamedRef(rev) => GitReference::NamedRef(rev),
        PixiGitReference::FullCommit(rev) => GitReference::FullCommit(rev),
        PixiGitReference::DefaultBranch => GitReference::DefaultBranch,
    }
}

pub fn into_pixi_reference(git_reference: GitReference) -> PixiReference {
    match git_reference {
        GitReference::Branch(branch) => PixiReference::Branch(branch.to_string()),
        GitReference::Tag(tag) => PixiReference::Tag(tag.to_string()),
        GitReference::ShortCommit(rev) => PixiReference::Rev(rev.to_string()),
        GitReference::BranchOrTag(rev) => PixiReference::Rev(rev.to_string()),
        GitReference::BranchOrTagOrCommit(rev) => PixiReference::Rev(rev.to_string()),
        GitReference::NamedRef(rev) => PixiReference::Rev(rev.to_string()),
        GitReference::FullCommit(rev) => PixiReference::Rev(rev.to_string()),
        GitReference::DefaultBranch => PixiReference::DefaultBranch,
    }
}

/// Convert a solved [`GitSourceDist`] into [`PinnedGitSpec`]
pub fn into_pinned_git_spec(dist: GitSourceDist) -> PinnedGitSpec {
    let reference = into_pixi_reference(dist.git.reference().clone());

    // Necessary to convert between our gitsha and uv gitsha.
    let git_sha = GitSha::from_str(
        &dist
            .git
            .precise()
            .expect("we expect it to be resolved")
            .to_string(),
    )
    .expect("we expect it to be a valid sha");

    let pinned_checkout = PinnedGitCheckout::new(
        git_sha,
        dist.subdirectory.map(|sd| sd.to_string_lossy().to_string()),
        reference,
    );

    PinnedGitSpec::new(dist.git.repository().clone(), pinned_checkout)
}
