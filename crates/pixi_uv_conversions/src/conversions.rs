use std::path::{Path, PathBuf};
use std::str::FromStr;

use pixi_git::sha::GitSha as PixiGitSha;
use pixi_git::url::RepositoryUrl;
use pixi_manifest::pypi::pypi_options::FindLinksUrlOrPath;
use pixi_manifest::pypi::pypi_options::{IndexStrategy, NoBuild, PypiOptions};
use pixi_record::{LockedGitUrl, PinnedGitCheckout, PinnedGitSpec};
use pixi_spec::GitReference as PixiReference;

use pixi_git::git::GitReference as PixiGitReference;

use pep440_rs::VersionSpecifiers;
use uv_configuration::BuildOptions;
use uv_distribution_types::{GitSourceDist, Index, IndexLocations, IndexUrl};
use uv_pep508::{InvalidNameError, PackageName, VerbatimUrl, VerbatimUrlError};
use uv_python::PythonEnvironment;

use crate::VersionError;

#[derive(thiserror::Error, Debug)]
pub enum ConvertFlatIndexLocationError {
    #[error("could not convert path to flat index location {1}")]
    VerbatimUrlError(#[source] VerbatimUrlError, PathBuf),
    #[error("base path is not absolute: {path}", path = .0.display())]
    NotAbsolute(PathBuf),
}

/// Convert PyPI options to build options
pub fn no_build_to_build_options(no_build: &NoBuild) -> Result<BuildOptions, InvalidNameError> {
    let uv_no_build = match no_build {
        NoBuild::None => uv_configuration::NoBuild::None,
        NoBuild::All => uv_configuration::NoBuild::All,
        NoBuild::Packages(ref vec) => uv_configuration::NoBuild::Packages(
            vec.iter()
                .map(|s| PackageName::new(s.to_string()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    Ok(BuildOptions::new(
        uv_configuration::NoBinary::default(),
        uv_no_build,
    ))
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

fn packages_to_build_isolation<'a>(
    names: Option<&'a [PackageName]>,
    python_environment: &'a PythonEnvironment,
) -> uv_types::BuildIsolation<'a> {
    if let Some(package_names) = names {
        uv_types::BuildIsolation::SharedPackage(python_environment, package_names)
    } else {
        uv_types::BuildIsolation::default()
    }
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

pub fn into_uv_git_reference(git_ref: PixiGitReference) -> uv_git::GitReference {
    match git_ref {
        PixiGitReference::Branch(branch) => uv_git::GitReference::Branch(branch),
        PixiGitReference::Tag(tag) => uv_git::GitReference::Tag(tag),
        PixiGitReference::ShortCommit(rev) | PixiGitReference::FullCommit(rev) => {
            uv_git::GitReference::BranchOrTagOrCommit(rev)
        }
        PixiGitReference::BranchOrTag(rev) => uv_git::GitReference::BranchOrTag(rev),
        PixiGitReference::BranchOrTagOrCommit(rev) => {
            uv_git::GitReference::BranchOrTagOrCommit(rev)
        }
        PixiGitReference::NamedRef(rev) => uv_git::GitReference::NamedRef(rev),
        PixiGitReference::DefaultBranch => uv_git::GitReference::DefaultBranch,
    }
}

pub fn into_uv_git_sha(git_sha: PixiGitSha) -> uv_git::GitOid {
    uv_git::GitOid::from_str(&git_sha.to_string()).expect("we expect it to be the same git sha")
}

pub fn into_pixi_reference(git_reference: uv_git::GitReference) -> PixiReference {
    match git_reference {
        uv_git::GitReference::Branch(branch) => PixiReference::Branch(branch.to_string()),
        uv_git::GitReference::Tag(tag) => PixiReference::Tag(tag.to_string()),
        uv_git::GitReference::BranchOrTag(rev) => PixiReference::Rev(rev.to_string()),
        uv_git::GitReference::BranchOrTagOrCommit(rev) => PixiReference::Rev(rev.to_string()),
        uv_git::GitReference::NamedRef(rev) => PixiReference::Rev(rev.to_string()),
        uv_git::GitReference::DefaultBranch => PixiReference::DefaultBranch,
    }
}

/// Convert a solved [`GitSourceDist`] into [`PinnedGitSpec`]
pub fn into_pinned_git_spec(dist: GitSourceDist) -> PinnedGitSpec {
    let reference = into_pixi_reference(dist.git.reference().clone());

    // Necessary to convert between our gitsha and uv gitsha.
    let git_sha = PixiGitSha::from_str(
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

/// Convert a locked git url into a parsed git url
/// [`LockedGitUrl`] is always recorded in the lock file and looks like this:
/// <git+https://git.example.com/MyProject.git?tag=v1.0&subdirectory=pkg_dir#1c4b2c7864a60ea169e091901fcde63a8d6fbfdc>
///
/// [`uv_pypi_types::ParsedGitUrl`] looks like this:
/// <git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir>
///
/// So we need to convert the locked git url into a parsed git url.
/// which is used in the uv crate.
pub fn to_parsed_git_url(
    locked_git_url: &LockedGitUrl,
) -> miette::Result<uv_pypi_types::ParsedGitUrl> {
    let git_source = PinnedGitCheckout::from_locked_url(locked_git_url)?;
    // Construct manually [`ParsedGitUrl`] from locked url.
    let parsed_git_url = uv_pypi_types::ParsedGitUrl::from_source(
        RepositoryUrl::new(&locked_git_url.to_url()).into(),
        into_uv_git_reference(git_source.reference.into()),
        Some(into_uv_git_sha(git_source.commit)),
        git_source.subdirectory.map(|s| PathBuf::from(s.as_str())),
    );

    Ok(parsed_git_url)
}

/// Converts from the open-source variant to the uv-specific variant,
/// these are incompatible types
pub fn to_uv_specifiers(
    specifiers: &VersionSpecifiers,
) -> Result<uv_pep440::VersionSpecifiers, uv_pep440::VersionSpecifiersParseError> {
    uv_pep440::VersionSpecifiers::from_str(specifiers.to_string().as_str())
}

pub fn to_requirements<'req>(
    requirements: impl Iterator<Item = &'req uv_pypi_types::Requirement>,
) -> Result<Vec<pep508_rs::Requirement>, crate::ConversionError> {
    let requirements: Result<Vec<pep508_rs::Requirement>, _> = requirements
        .map(|requirement| {
            let requirement: uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl> =
                uv_pep508::Requirement::from(requirement.clone());
            pep508_rs::Requirement::from_str(&requirement.to_string())
                .map_err(crate::Pep508Error::Pep508Error)
        })
        .collect();

    Ok(requirements?)
}

/// Convert back to PEP508 without the VerbatimParsedUrl
/// We need this function because we need to convert to the introduced
/// `VerbatimParsedUrl` back to crates.io `VerbatimUrl`, for the locking
pub fn convert_uv_requirements_to_pep508<'req>(
    requires_dist: impl Iterator<Item = &'req uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl>>,
) -> Result<Vec<pep508_rs::Requirement>, crate::ConversionError> {
    // Convert back top PEP508 Requirement<VerbatimUrl>
    let requirements: Result<Vec<pep508_rs::Requirement>, _> = requires_dist
        .map(|r| {
            let requirement = r.to_string();
            pep508_rs::Requirement::from_str(&requirement).map_err(crate::Pep508Error::Pep508Error)
        })
        .collect();

    Ok(requirements?)
}

/// Converts `uv_normalize::PackageName` to `pep508_rs::PackageName`
pub fn to_normalize(
    normalise: &uv_normalize::PackageName,
) -> Result<pep508_rs::PackageName, crate::ConversionError> {
    Ok(pep508_rs::PackageName::from_str(normalise.as_str())
        .map_err(crate::NameError::PepNameError)?)
}

/// Converts `pe508::PackageName` to  `uv_normalize::PackageName`
pub fn to_uv_normalize(
    normalise: &pep508_rs::PackageName,
) -> Result<uv_normalize::PackageName, crate::ConversionError> {
    Ok(
        uv_normalize::PackageName::from_str(normalise.to_string().as_str())
            .map_err(crate::NameError::UvNameError)?,
    )
}

/// Converts `pep508_rs::ExtraName` to `uv_normalize::ExtraName`
pub fn to_uv_extra_name(
    extra_name: &pep508_rs::ExtraName,
) -> Result<uv_normalize::ExtraName, crate::ConversionError> {
    Ok(
        uv_normalize::ExtraName::from_str(extra_name.to_string().as_str())
            .map_err(crate::NameError::UvExtraNameError)?,
    )
}

/// Converts `uv_normalize::ExtraName` to `pep508_rs::ExtraName`
pub fn to_extra_name(
    extra_name: &uv_normalize::ExtraName,
) -> Result<pep508_rs::ExtraName, crate::ConversionError> {
    Ok(
        pep508_rs::ExtraName::from_str(extra_name.to_string().as_str())
            .map_err(crate::NameError::PepExtraNameError)?,
    )
}

/// Converts `pep440_rs::Version` to `uv_pep440::Version`
pub fn to_uv_version(
    version: &pep440_rs::Version,
) -> Result<uv_pep440::Version, crate::ConversionError> {
    Ok(
        uv_pep440::Version::from_str(version.to_string().as_str())
            .map_err(VersionError::UvError)?,
    )
}

/// Converts `pep508_rs::MarkerTree` to `uv_pep508::MarkerTree`
pub fn to_uv_marker_tree(
    marker_tree: &pep508_rs::MarkerTree,
) -> Result<uv_pep508::MarkerTree, crate::ConversionError> {
    let serialized = marker_tree.try_to_string();
    if let Some(serialized) = serialized {
        Ok(uv_pep508::MarkerTree::from_str(serialized.as_str())
            .map_err(crate::Pep508Error::UvPep508)?)
    } else {
        Ok(uv_pep508::MarkerTree::default())
    }
}

/// Converts `uv_pep508::MarkerTree` to `pep508_rs::MarkerTree`
pub fn to_marker_environment(
    marker_env: &uv_pep508::MarkerEnvironment,
) -> Result<pep508_rs::MarkerEnvironment, crate::ConversionError> {
    let serde_str = serde_json::to_string(marker_env).expect("its valid");
    serde_json::from_str(&serde_str).map_err(crate::ConversionError::MarkerEnvironmentSerialization)
}

/// Converts `pep440_rs::VersionSpecifiers` to `uv_pep440::VersionSpecifiers`
pub fn to_uv_version_specifiers(
    version_specifier: &pep440_rs::VersionSpecifiers,
) -> Result<uv_pep440::VersionSpecifiers, crate::ConversionError> {
    Ok(
        uv_pep440::VersionSpecifiers::from_str(&version_specifier.to_string())
            .map_err(crate::VersionSpecifiersError::UvVersionError)?,
    )
}

/// Converts `uv_pep440::VersionSpecifiers` to `pep440_rs::VersionSpecifiers`
pub fn to_version_specifiers(
    version_specifier: &uv_pep440::VersionSpecifiers,
) -> Result<pep440_rs::VersionSpecifiers, crate::ConversionError> {
    Ok(
        pep440_rs::VersionSpecifiers::from_str(&version_specifier.to_string())
            .map_err(crate::VersionSpecifiersError::PepVersionError)?,
    )
}

/// Converts trusted_host `string` to `uv_configuration::TrustedHost`
pub fn to_uv_trusted_host(
    trusted_host: &str,
) -> Result<uv_configuration::TrustedHost, crate::ConversionError> {
    Ok(uv_configuration::TrustedHost::from_str(trusted_host)?)
}
