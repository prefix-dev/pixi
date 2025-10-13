use crate::BinarySpec;
use itertools::Either;
use rattler_conda_types::{NamelessMatchSpec, package::ArchiveType};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::{fmt::Display, path::Path};
use url::Url;

/// A specification of a package from a URL. This is used to represent both
/// source and binary packages.
#[serde_as]
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct UrlSpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl UrlSpec {
    /// Converts this instance into a [`NamelessMatchSpec`] if the URL points to
    /// a binary package.
    #[allow(clippy::result_large_err)]
    pub fn try_into_nameless_match_spec(self) -> Result<NamelessMatchSpec, Self> {
        if self.is_binary() {
            Ok(NamelessMatchSpec {
                url: Some(self.url),
                md5: self.md5,
                sha256: self.sha256,
                ..NamelessMatchSpec::default()
            })
        } else {
            Err(self)
        }
    }

    /// Converts this instance into a [`UrlSourceSpec`] if the URL points to a
    /// source package. Otherwise, returns this instance unmodified.
    #[allow(clippy::result_large_err)]
    pub fn try_into_source_url(self) -> Result<UrlSourceSpec, Self> {
        if self.is_binary() {
            Err(self)
        } else {
            Ok(UrlSourceSpec {
                url: self.url,
                md5: self.md5,
                sha256: self.sha256,
            })
        }
    }

    /// Converts this instance into a [`UrlSourceSpec`] if the URL points to a
    /// source package. Or to a [`UrlBinarySpec`] otherwise.
    pub fn into_source_or_binary(self) -> Either<UrlSourceSpec, UrlBinarySpec> {
        if self.is_binary() {
            Either::Right(UrlBinarySpec {
                url: self.url,
                md5: self.md5,
                sha256: self.sha256,
            })
        } else {
            Either::Left(UrlSourceSpec {
                url: self.url,
                md5: self.md5,
                sha256: self.sha256,
            })
        }
    }

    /// Returns true if the URL points to a binary package.
    ///
    /// This is determined by checking if the URL path has a known binary archive
    /// extension (e.g. `.tar.bz2`, `.conda`).
    pub fn is_binary(&self) -> bool {
        ArchiveType::try_from(Path::new(self.url.path())).is_some()
    }
}

impl Display for UrlSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)?;
        if let Some(md5) = &self.md5 {
            write!(f, " md5={:x}", md5)?;
        }
        if let Some(sha256) = &self.sha256 {
            write!(f, " sha256={:x}", sha256)?;
        }
        Ok(())
    }
}

/// A specification of a source archive from a URL.
#[serde_as]
#[derive(Debug, Clone, Hash, Eq, PartialEq, serde::Serialize)]
pub struct UrlSourceSpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the archive
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash<rattler_digest::Md5>>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the archive
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash<rattler_digest::Sha256>>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Hash>,
}

impl Display for UrlSourceSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)?;
        if let Some(md5) = &self.md5 {
            write!(f, " md5={:x}", md5)?;
        }
        if let Some(sha256) = &self.sha256 {
            write!(f, " sha256={:x}", sha256)?;
        }
        Ok(())
    }
}

impl From<UrlSourceSpec> for UrlSpec {
    fn from(value: UrlSourceSpec) -> Self {
        Self {
            url: value.url,
            md5: value.md5,
            sha256: value.sha256,
        }
    }
}

/// A specification of a source archive from a URL.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
pub struct UrlBinarySpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the archive
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the archive
    pub sha256: Option<Sha256Hash>,
}

impl From<UrlBinarySpec> for UrlSpec {
    fn from(value: UrlBinarySpec) -> Self {
        Self {
            url: value.url,
            md5: value.md5,
            sha256: value.sha256,
        }
    }
}

impl From<UrlBinarySpec> for NamelessMatchSpec {
    fn from(value: UrlBinarySpec) -> Self {
        NamelessMatchSpec {
            url: Some(value.url),
            md5: value.md5,
            sha256: value.sha256,
            ..NamelessMatchSpec::default()
        }
    }
}

impl From<UrlBinarySpec> for BinarySpec {
    fn from(value: UrlBinarySpec) -> Self {
        Self::Url(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_binary() {
        // Test binary archive URLs supported by ArchiveType
        let binary_urls = vec![
            "https://conda.anaconda.org/conda-forge/linux-64/package.tar.bz2",
            "https://conda.anaconda.org/conda-forge/linux-64/package.conda",
            "file:///path/to/package.tar.bz2",
            "file:///path/to/package.conda",
        ];

        for url_str in binary_urls {
            let url = Url::parse(url_str).unwrap();
            let spec = UrlSpec {
                url: url.clone(),
                md5: None,
                sha256: None,
            };
            assert!(
                spec.is_binary(),
                "Expected {} to be identified as a binary archive",
                url_str
            );
        }

        // Test non-binary URLs (including unsupported archive formats)
        let non_binary_urls = vec![
            "https://github.com/user/repo/archive/v1.0.0.zip",
            "https://github.com/user/repo/archive/v1.0.0.tar.gz",
            "https://example.com/source.tar.xz",
            "https://pypi.org/package/source.whl",
            "https://example.com/source.tar",
        ];

        for url_str in non_binary_urls {
            let url = Url::parse(url_str).unwrap();
            let spec = UrlSpec {
                url: url.clone(),
                md5: None,
                sha256: None,
            };
            assert!(
                !spec.is_binary(),
                "Expected {} to NOT be identified as a binary archive",
                url_str
            );
        }
    }

    #[test]
    fn test_into_source_or_binary() {
        // Binary URL should return Right (binary)
        let binary_url = Url::parse("https://conda.anaconda.org/package.tar.bz2").unwrap();
        let binary_spec = UrlSpec {
            url: binary_url,
            md5: None,
            sha256: None,
        };
        match binary_spec.into_source_or_binary() {
            Either::Right(_) => {}
            Either::Left(_) => panic!("Expected binary URL to return Right variant"),
        }

        // Non-binary URL should return Left (source)
        let source_url = Url::parse("https://github.com/user/repo/archive/v1.0.0.zip").unwrap();
        let source_spec = UrlSpec {
            url: source_url,
            md5: None,
            sha256: None,
        };
        match source_spec.into_source_or_binary() {
            Either::Left(_) => {}
            Either::Right(_) => panic!("Expected source URL to return Left variant"),
        }
    }

    #[test]
    fn test_try_into_source_url() {
        // Binary URL should return Err with original spec
        let binary_url = Url::parse("https://conda.anaconda.org/package.conda").unwrap();
        let binary_spec = UrlSpec {
            url: binary_url,
            md5: None,
            sha256: None,
        };
        assert!(
            binary_spec.try_into_source_url().is_err(),
            "Expected binary URL to fail conversion to source URL"
        );

        // Non-binary URL should return Ok with source spec
        let source_url = Url::parse("https://example.com/source.zip").unwrap();
        let source_spec = UrlSpec {
            url: source_url,
            md5: None,
            sha256: None,
        };
        assert!(
            source_spec.try_into_source_url().is_ok(),
            "Expected non-binary URL to succeed conversion to source URL"
        );
    }
}
