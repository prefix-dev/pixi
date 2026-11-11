use rattler_conda_types::PackageUrl;

/// The result of asking a mapping source to derive purls for a record.
pub(crate) enum DerivationOutcome {
    /// This source does not know about the record; another source may be tried.
    NotApplicable,
    /// This source knows the record maps to no PyPI package.
    NoPurls,
    /// This source derived one or more purls for the record.
    Purls(Vec<PackageUrl>),
}

impl DerivationOutcome {
    pub(crate) fn is_not_applicable(&self) -> bool {
        matches!(self, Self::NotApplicable)
    }

    pub(crate) fn into_purls(self) -> Option<Vec<PackageUrl>> {
        match self {
            Self::NotApplicable => None,
            Self::NoPurls => Some(Vec::new()),
            Self::Purls(purls) => Some(purls),
        }
    }
}
