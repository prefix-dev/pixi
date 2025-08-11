use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::GitUrlWithPrefix;
use miette::IntoDiagnostic;
use pep440_rs::VersionSpecifiers;
use pixi_git::{git::GitReference as PixiGitReference, sha::GitSha as PixiGitSha};
use pixi_manifest::pypi::pypi_options::{
    FindLinksUrlOrPath, IndexStrategy, NoBinary, NoBuild, NoBuildIsolation, PypiOptions,
};
use pixi_record::{LockedGitUrl, PinnedGitCheckout, PinnedGitSpec};
use pixi_spec::GitReference as PixiReference;
use uv_configuration::BuildOptions;
use uv_distribution_types::{GitSourceDist, Index, IndexLocations, IndexUrl};
use uv_pep508::{InvalidNameError, PackageName, VerbatimUrl, VerbatimUrlError};
use uv_python::PythonEnvironment;
use uv_redacted::DisplaySafeUrl;

use crate::{ConversionError, VersionError};

#[derive(thiserror::Error, Debug)]
pub enum ConvertFlatIndexLocationError {
    #[error("could not convert path to flat index location {1}")]
    VerbatimUrlError(#[source] VerbatimUrlError, PathBuf),
    #[error("base path is not absolute: {path}", path = .0.display())]
    NotAbsolute(PathBuf),
}

/// Convert PyPI options to build options
pub fn pypi_options_to_build_options(
    no_build: &NoBuild,
    no_binary: &NoBinary,
) -> Result<BuildOptions, InvalidNameError> {
    let uv_no_build = match no_build {
        NoBuild::None => uv_configuration::NoBuild::None,
        NoBuild::All => uv_configuration::NoBuild::All,
        NoBuild::Packages(vec) => uv_configuration::NoBuild::Packages(
            vec.iter()
                .map(|s| PackageName::from_str(s.as_ref()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let uv_no_binary = match no_binary {
        NoBinary::None => uv_configuration::NoBinary::None,
        NoBinary::All => uv_configuration::NoBinary::All,
        NoBinary::Packages(vec) => uv_configuration::NoBinary::Packages(
            vec.iter()
                .map(|s| PackageName::from_str(s.as_ref()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };

    Ok(BuildOptions::new(uv_no_binary, uv_no_build))
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
        .map(DisplaySafeUrl::from)
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
                .map(DisplaySafeUrl::from)
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
                FindLinksUrlOrPath::Url(url) => Ok(VerbatimUrl::from_url(url.clone().into())),
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
        .map(DisplaySafeUrl::from)
        .map(VerbatimUrl::from_url)
        .map(IndexUrl::from)
        .map(Index::from_index_url)
        .into_iter();
    let extra_indexes = indexes
        .indexes
        .iter()
        .skip(1)
        .cloned()
        .map(DisplaySafeUrl::from)
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
            rattler_lock::FindLinksUrlOrPath::Url(url) => {
                Ok(VerbatimUrl::from_url(url.clone().into()))
            }
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

#[derive(Clone)]
pub enum BuildIsolation {
    /// No build isolation
    Isolated,
    /// Build isolation with a shared environment
    Shared,
    /// Build isolation with a shared environment and a list of package names
    SharedPackage(Vec<PackageName>),
}

impl BuildIsolation {
    pub fn to_uv<'a>(&'a self, python_env: &'a PythonEnvironment) -> uv_types::BuildIsolation<'a> {
        match self {
            BuildIsolation::Isolated => uv_types::BuildIsolation::Isolated,
            BuildIsolation::Shared => uv_types::BuildIsolation::Shared(python_env),
            BuildIsolation::SharedPackage(packages) => {
                uv_types::BuildIsolation::SharedPackage(python_env, packages)
            }
        }
    }

    pub fn to_uv_with<'a, F: FnOnce() -> &'a PythonEnvironment>(
        &'a self,
        get_env: F,
    ) -> uv_types::BuildIsolation<'a> {
        match self {
            BuildIsolation::Isolated => uv_types::BuildIsolation::Isolated,
            BuildIsolation::Shared => uv_types::BuildIsolation::Shared(get_env()),
            BuildIsolation::SharedPackage(packages) => {
                uv_types::BuildIsolation::SharedPackage(get_env(), packages)
            }
        }
    }
}

impl TryFrom<NoBuildIsolation> for BuildIsolation {
    type Error = ConversionError;

    fn try_from(no_build: NoBuildIsolation) -> Result<Self, Self::Error> {
        Ok(match no_build {
            NoBuildIsolation::All => BuildIsolation::Shared,
            NoBuildIsolation::Packages(packages) if packages.is_empty() => BuildIsolation::Isolated,
            NoBuildIsolation::Packages(packages) => BuildIsolation::SharedPackage(
                packages
                    .into_iter()
                    .map(|pkg| to_uv_normalize(&pkg))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
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

pub fn into_uv_git_reference(git_ref: PixiGitReference) -> uv_git_types::GitReference {
    match git_ref {
        PixiGitReference::Branch(branch) => uv_git_types::GitReference::Branch(branch),
        PixiGitReference::Tag(tag) => uv_git_types::GitReference::Tag(tag),
        PixiGitReference::ShortCommit(rev) | PixiGitReference::FullCommit(rev) => {
            uv_git_types::GitReference::BranchOrTagOrCommit(rev)
        }
        PixiGitReference::BranchOrTag(rev) => uv_git_types::GitReference::BranchOrTag(rev),
        PixiGitReference::BranchOrTagOrCommit(rev) => {
            uv_git_types::GitReference::BranchOrTagOrCommit(rev)
        }
        PixiGitReference::NamedRef(rev) => uv_git_types::GitReference::NamedRef(rev),
        PixiGitReference::DefaultBranch => uv_git_types::GitReference::DefaultBranch,
    }
}

pub fn into_uv_git_sha(git_sha: PixiGitSha) -> uv_git_types::GitOid {
    uv_git_types::GitOid::from_str(&git_sha.to_string())
        .expect("we expect it to be the same git sha")
}

pub fn into_pixi_reference(git_reference: uv_git_types::GitReference) -> PixiReference {
    match git_reference {
        uv_git_types::GitReference::Branch(branch) => PixiReference::Branch(branch.to_string()),
        uv_git_types::GitReference::Tag(tag) => PixiReference::Tag(tag.to_string()),
        uv_git_types::GitReference::BranchOrTag(rev) => PixiReference::Rev(rev.to_string()),
        uv_git_types::GitReference::BranchOrTagOrCommit(rev) => PixiReference::Rev(rev.to_string()),
        uv_git_types::GitReference::NamedRef(rev) => PixiReference::Rev(rev.to_string()),
        uv_git_types::GitReference::DefaultBranch => PixiReference::DefaultBranch,
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

    PinnedGitSpec::new(dist.git.repository().clone().into(), pinned_checkout)
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
        uv_git_types::GitUrl::from_fields(
            {
                let mut url = locked_git_url.to_url();
                // Locked git url contains query parameters and fragments
                // so we need to clean it to a base repository URL
                url.set_fragment(None);
                url.set_query(None);

                let git_url = GitUrlWithPrefix::from(&url);
                git_url.to_display_safe_url()
            },
            into_uv_git_reference(git_source.reference.into()),
            Some(into_uv_git_sha(git_source.commit)),
        )
        .into_diagnostic()?,
        git_source
            .subdirectory
            .map(|s| PathBuf::from(s.as_str()).into_boxed_path()),
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
    requirements: impl Iterator<Item = &'req uv_distribution_types::Requirement>,
) -> Result<Vec<pep508_rs::Requirement>, crate::ConversionError> {
    let requirements: Result<Vec<pep508_rs::Requirement>, _> = requirements
        .map(|requirement| {
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

/// Converts a date to a `uv_resolver::ExcludeNewer`
/// since 0.8.2 uv also allows this per package,
/// but we only support the global one for now
pub fn to_exclude_newer(exclude_newer: chrono::DateTime<chrono::Utc>) -> uv_resolver::ExcludeNewer {
    let seconds_since_epoch = exclude_newer.timestamp();
    let nanoseconds = exclude_newer.timestamp_subsec_nanos();
    let timestamp = jiff::Timestamp::new(seconds_since_epoch, nanoseconds as _).unwrap_or(
        if seconds_since_epoch < 0 {
            jiff::Timestamp::MIN
        } else {
            jiff::Timestamp::MAX
        },
    );
    // Will convert into a global ExcludeNewer
    // ..into is needed to convert into the uv timestamp type
    uv_resolver::ExcludeNewer::global(timestamp.into())
}
