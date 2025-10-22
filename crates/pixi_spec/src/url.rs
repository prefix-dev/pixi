use crate::BinarySpec;
use itertools::Either;
use rattler_conda_types::{NamelessMatchSpec, package::ArchiveIdentifier};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::fmt::Display;
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
    pub fn is_binary(&self) -> bool {
        ArchiveIdentifier::try_from_url(&self.url).is_some()
    }
}

impl Display for UrlSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)?;
        if let Some(md5) = &self.md5 {
            write!(f, " md5={md5:x}")?;
        }
        if let Some(sha256) = &self.sha256 {
            write!(f, " sha256={sha256:x}")?;
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
            write!(f, " md5={md5:x}")?;
        }
        if let Some(sha256) = &self.sha256 {
            write!(f, " sha256={sha256:x}")?;
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
