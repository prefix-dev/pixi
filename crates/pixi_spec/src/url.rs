use rattler_conda_types::{package::ArchiveIdentifier, NamelessMatchSpec};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde_with::serde_as;
use url::Url;

/// A specification of a package from a URL. This is used to represent both
/// source and binary packages.
#[serde_as]
#[derive(Debug, Clone, Hash, Eq, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
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

    /// Returns true if the URL points to a binary package.
    pub fn is_binary(&self) -> bool {
        ArchiveIdentifier::try_from_url(&self.url).is_some()
    }
}

/// A specification of a source archive from a URL.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct UrlSourceSpec {
    /// The URL of the package
    pub url: Url,

    /// The md5 hash of the archive
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the archive
    pub sha256: Option<Sha256Hash>,
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
