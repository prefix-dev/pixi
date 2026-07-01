use rattler_conda_types::PackageUrl;

/// Identifies the concrete mechanism that derived a PyPI purl.
///
/// This is intentionally different from [`crate::PurlDerivationMode`]:
/// `PurlDerivationMode` describes the user-selected mapping mode, while this enum
/// describes the specific resolver that produced a purl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurlDerivationSource {
    /// prefix.dev hash mapping, looked up by package SHA256.
    PrefixHashMapping,
    /// prefix.dev compressed name mapping, looked up by conda package name.
    PrefixCompressedMapping,
    /// Project/user-defined per-channel mapping.
    ProjectDefinedMapping,
    /// Last-resort heuristic that assumes the conda name is the PyPI name.
    ///
    /// This source is not encoded as a `source` qualifier in generated purls.
    SameName,
}

impl PurlDerivationSource {
    pub fn as_str(&self) -> &str {
        match self {
            PurlDerivationSource::PrefixHashMapping => "hash-mapping",
            PurlDerivationSource::PrefixCompressedMapping => "compressed-mapping",
            PurlDerivationSource::ProjectDefinedMapping => "project-defined-mapping",
            PurlDerivationSource::SameName => "same-name-heuristic",
        }
    }

    pub(crate) fn purl_qualifier(self) -> Option<&'static str> {
        match self {
            PurlDerivationSource::PrefixHashMapping => Some("hash-mapping"),
            PurlDerivationSource::PrefixCompressedMapping => Some("compressed-mapping"),
            PurlDerivationSource::ProjectDefinedMapping => Some("project-defined-mapping"),
            PurlDerivationSource::SameName => None,
        }
    }
}

/// Builds a PyPI package URL, optionally tagging it with the derivation source.
pub(crate) fn pypi_purl(
    name: impl Into<String>,
    source: Option<PurlDerivationSource>,
) -> PackageUrl {
    let mut builder = PackageUrl::builder(String::from("pypi"), name.into());

    if let Some(source) = source.and_then(PurlDerivationSource::purl_qualifier) {
        builder = builder
            .with_qualifier("source", source)
            .expect("valid qualifier");
    }

    builder.build().expect("valid pypi package url")
}
