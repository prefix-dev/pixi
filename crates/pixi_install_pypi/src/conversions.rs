use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use pixi_consts::consts;
use pixi_record::LockedGitUrl;
use pixi_uv_conversions::{
    ConversionError, to_parsed_git_url, to_uv_normalize, to_uv_version, to_uv_version_specifiers,
};
use rattler_lock::{PackageHashes, PypiPackageData, UrlOrPath};
use url::Url;
use uv_distribution_filename::DistExtension;
use uv_distribution_filename::{ExtensionError, SourceDistExtension, WheelFilename};
use uv_distribution_types::{
    BuiltDist, Dist, IndexUrl, RegistryBuiltDist, RegistryBuiltWheel, RegistrySourceDist,
    SourceDist, UrlString,
};
use uv_pypi_types::{HashAlgorithm, HashDigest, ParsedUrl, ParsedUrlError, VerbatimParsedUrl};

use super::utils::{is_direct_url, strip_direct_scheme};

/// Converts our locked data to a file
pub fn locked_data_to_file(
    url: &Url,
    hash: Option<&PackageHashes>,
    filename: &str,
    requires_python: Option<pep440_rs::VersionSpecifiers>,
) -> Result<uv_distribution_types::File, ConversionError> {
    let url = uv_distribution_types::FileLocation::AbsoluteUrl(UrlString::from(
        uv_redacted::DisplaySafeUrl::from(url.clone()),
    ));

    // Convert PackageHashes to uv hashes
    let hashes = if let Some(hash) = hash {
        match hash {
            rattler_lock::PackageHashes::Md5(md5) => vec![HashDigest {
                algorithm: HashAlgorithm::Md5,
                digest: format!("{:x}", md5).into(),
            }],
            rattler_lock::PackageHashes::Sha256(sha256) => vec![HashDigest {
                algorithm: HashAlgorithm::Sha256,
                digest: format!("{:x}", sha256).into(),
            }],
            rattler_lock::PackageHashes::Md5Sha256(md5, sha256) => vec![
                HashDigest {
                    algorithm: HashAlgorithm::Md5,
                    digest: format!("{:x}", md5).into(),
                },
                HashDigest {
                    algorithm: HashAlgorithm::Sha256,
                    digest: format!("{:x}", sha256).into(),
                },
            ],
        }
    } else {
        vec![]
    };

    let uv_requires_python = requires_python
        .map(|inside| to_uv_version_specifiers(&inside))
        .transpose()?;

    Ok(uv_distribution_types::File {
        filename: filename.into(),
        dist_info_metadata: false,
        hashes: hashes.into(),
        requires_python: uv_requires_python,
        upload_time_utc_ms: None,
        yanked: None,
        size: None,
        url,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ConvertToUvDistError {
    #[error("error creating ParsedUrl")]
    ParseUrl(#[from] Box<ParsedUrlError>),
    #[error("error creating uv Dist from url")]
    Uv(#[from] uv_distribution_types::Error),
    #[error("error constructing verbatim url")]
    VerbatimUrl(#[from] uv_pep508::VerbatimUrlError),
    #[error("error extracting extension from {1}")]
    Extension(#[source] ExtensionError, String),
    #[error("error parsing locked git url {0} {1}")]
    LockedUrl(String, String),
    #[error("invalid package name: {0}")]
    InvalidPackageName(String),

    #[error(transparent)]
    UvPepTypes(#[from] ConversionError),
}

/// Convert from a PypiPackageData to a uv [`distribution_types::Dist`]
pub fn convert_to_dist(
    pkg: &PypiPackageData,
    lock_file_dir: &Path,
) -> Result<Dist, ConvertToUvDistError> {
    // Figure out if it is a url from the registry or a direct url
    let dist = match &pkg.location {
        UrlOrPath::Url(url) if is_direct_url(url.scheme()) => {
            let url_without_direct = strip_direct_scheme(url);
            let pkg_name = to_uv_normalize(&pkg.name)?;

            if LockedGitUrl::is_locked_git_url(&url_without_direct) {
                let locked_git_url = LockedGitUrl::new(url_without_direct.clone().into_owned());
                let parsed_git_url = to_parsed_git_url(&locked_git_url).map_err(|err| {
                    ConvertToUvDistError::LockedUrl(
                        err.to_string(),
                        locked_git_url.to_url().to_string(),
                    )
                })?;

                Dist::from_url(
                    pkg_name,
                    VerbatimParsedUrl {
                        parsed_url: ParsedUrl::Git(parsed_git_url),
                        verbatim: uv_pep508::VerbatimUrl::from(url_without_direct.into_owned()),
                    },
                )?
            } else {
                Dist::from_url(
                    pkg_name,
                    VerbatimParsedUrl {
                        parsed_url: ParsedUrl::try_from(uv_redacted::DisplaySafeUrl::from(
                            url_without_direct.clone().into_owned(),
                        ))
                        .map_err(Box::new)?,
                        verbatim: uv_pep508::VerbatimUrl::from(url_without_direct.into_owned()),
                    },
                )?
            }
        }
        UrlOrPath::Url(url) => {
            // We consider it to be a registry url
            // Extract last component from registry url
            // should be something like `package-0.1.0-py3-none-any.whl`
            let filename_raw = url
                .path_segments()
                .expect("url should have path segments")
                .next_back()
                .expect("url should have at least one path segment");

            // Decode the filename to avoid issues with the HTTP coding like `%2B` to `+`
            let filename_decoded =
                percent_encoding::percent_decode_str(filename_raw).decode_utf8_lossy();

            // Now we can convert the locked data to a [`distribution_types::File`]
            // which is essentially the file information for a wheel or sdist
            let file = locked_data_to_file(
                url,
                pkg.hash.as_ref(),
                filename_decoded.as_ref(),
                pkg.requires_python.clone(),
            )?;
            // Recreate the filename from the extracted last component
            // If this errors this is not a valid wheel filename
            // and we should consider it a sdist
            let filename = WheelFilename::from_str(filename_decoded.as_ref());
            if let Ok(filename) = filename {
                Dist::Built(BuiltDist::Registry(RegistryBuiltDist {
                    wheels: vec![RegistryBuiltWheel {
                        filename,
                        file: Box::new(file),
                        // This should be fine because currently it is only used for caching
                        // When upgrading uv and running into problems we would need to sort this
                        // out but it would require adding the indexes to
                        // the lock file
                        index: IndexUrl::Pypi(Arc::new(uv_pep508::VerbatimUrl::from_url(
                            uv_redacted::DisplaySafeUrl::from(
                                consts::DEFAULT_PYPI_INDEX_URL.clone(),
                            ),
                        ))),
                    }],
                    best_wheel_index: 0,
                    sdist: None,
                }))
            } else {
                let pkg_name = to_uv_normalize(&pkg.name)?;
                let pkg_version = to_uv_version(&pkg.version)?;
                Dist::Source(SourceDist::Registry(RegistrySourceDist {
                    name: pkg_name,
                    version: pkg_version,
                    file: Box::new(file),
                    // This should be fine because currently it is only used for caching
                    index: IndexUrl::Pypi(Arc::new(uv_pep508::VerbatimUrl::from_url(
                        uv_redacted::DisplaySafeUrl::from(consts::DEFAULT_PYPI_INDEX_URL.clone()),
                    ))),
                    // I don't think this really matters for the install
                    wheels: vec![],
                    ext: SourceDistExtension::from_path(Path::new(filename_raw)).map_err(|e| {
                        ConvertToUvDistError::Extension(e, filename_raw.to_string())
                    })?,
                }))
            }
        }
        UrlOrPath::Path(path) => {
            let native_path = Path::new(path.as_str());
            let abs_path = if path.is_absolute() {
                native_path.to_path_buf()
            } else {
                lock_file_dir.join(native_path)
            };

            let absolute_url = uv_pep508::VerbatimUrl::from_absolute_path(&abs_path)?;
            let pkg_name =
                uv_normalize::PackageName::from_str(pkg.name.as_ref()).expect("should be correct");
            if abs_path.is_dir() {
                Dist::from_directory_url(
                    pkg_name,
                    absolute_url,
                    &abs_path,
                    Some(pkg.editable),
                    Some(false),
                )?
            } else {
                Dist::from_file_url(
                    pkg_name,
                    absolute_url,
                    &abs_path,
                    DistExtension::from_path(&abs_path).map_err(|e| {
                        ConvertToUvDistError::Extension(e, abs_path.to_string_lossy().to_string())
                    })?,
                )?
            }
        }
    };

    Ok(dist)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use pep440_rs::Version;
    use rattler_lock::{PypiPackageData, UrlOrPath};
    use uv_distribution_types::RemoteSource;

    use super::convert_to_dist;

    #[test]
    /// Create locked pypi data, pass this into the convert_to_dist function
    fn convert_special_chars_wheelname_to_dist() {
        // Create url with special characters
        let wheel = "torch-2.3.0%2Bcu121-cp312-cp312-win_amd64.whl";
        let url = format!("https://example.com/{}", wheel).parse().unwrap();
        // Pass into locked data
        let locked = PypiPackageData {
            name: "torch".parse().unwrap(),
            version: Version::from_str("2.3.0+cu121").unwrap(),
            location: UrlOrPath::Url(url),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
            editable: false,
        };

        // Convert the locked data to a uv dist
        // check if it does not panic
        let dist = convert_to_dist(&locked, &PathBuf::new())
            .expect("could not convert wheel with special chars to dist");

        // Check if the dist is a built dist
        assert!(!dist.filename().unwrap().contains("%2B"));
    }
}
