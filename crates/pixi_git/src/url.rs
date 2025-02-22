/// Derived from `uv-cache-key` implementation
/// Source: https://github.com/astral-sh/uv/blob/4b8cc3e29e4c2a6417479135beaa9783b05195d3/crates/uv-cache-key/src/canonical_url.rs
/// The main purpose of this module is to provide a wrapper around `Url` which represents a "canonical" version of an original URL.
/// This is used when some `GitUrls` are to be cached on the filesystem, and we want to ensure that URLs that are semantically equivalent.
use url::Url;

/// A wrapper around `Url` which represents a "canonical" version of an original URL.
///
/// A "canonical" url is only intended for internal comparison purposes. It's to help paper over
/// mistakes such as depending on `github.com/foo/bar` vs. `github.com/foo/bar.git`.
///
/// This is **only** for internal purposes and provides no means to actually read the underlying
/// string value of the `Url` it contains. This is intentional, because all fetching should still
/// happen within the context of the original URL.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct CanonicalUrl(Url);

impl CanonicalUrl {
    pub fn new(url: &Url) -> Self {
        let mut url = url.clone();

        // If the URL cannot be a base, then it's not a valid URL anyway.
        if url.cannot_be_a_base() {
            return Self(url);
        }

        // If the URL has no host, then it's not a valid URL anyway.
        if !url.has_host() {
            return Self(url);
        }

        // Strip credentials.
        let _ = url.set_password(None);

        // Only stripping the username if the scheme is not `ssh`. This is because `ssh` URLs should
        // have the `git` username.
        if !url.scheme().contains("ssh") {
            let _ = url.set_username("");
        }

        // Strip a trailing slash.
        if url.path().ends_with('/') {
            url.path_segments_mut()
                .expect("url should be a base")
                .pop_if_empty();
        }

        // For GitHub URLs specifically, just lower-case everything. GitHub
        // treats both the same, but they hash differently, and we're gonna be
        // hashing them. This wants a more general solution, and also we're
        // almost certainly not using the same case conversion rules that GitHub
        // does. (See issue #84)
        if url.host_str() == Some("github.com") {
            url.set_scheme(url.scheme().to_lowercase().as_str())
                .expect("we should be able to set scheme");
            let path = url.path().to_lowercase();
            url.set_path(&path);
        }

        // Repos can generally be accessed with or without `.git` extension.
        if let Some((prefix, suffix)) = url.path().rsplit_once('@') {
            // Ex) `git+https://github.com/pypa/sample-namespace-packages.git@2.0.0`
            let needs_chopping = std::path::Path::new(prefix)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("git"));
            if needs_chopping {
                let prefix = &prefix[..prefix.len() - 4];
                url.set_path(&format!("{prefix}@{suffix}"));
            }
        } else {
            // Ex) `git+https://github.com/pypa/sample-namespace-packages.git`
            let needs_chopping = std::path::Path::new(url.path())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("git"));
            if needs_chopping {
                let last = {
                    let last = url.path_segments().unwrap().next_back().unwrap();
                    last[..last.len() - 4].to_owned()
                };
                url.path_segments_mut().unwrap().pop().push(&last);
            }
        }

        Self(url)
    }

    pub fn parse(url: &str) -> Result<Self, url::ParseError> {
        Ok(Self::new(&Url::parse(url)?))
    }
}

/// Like [`CanonicalUrl`], but attempts to represent an underlying source repository, abstracting
/// away details like the specific commit or branch, or the subdirectory to build within the
/// repository.
///
/// For example, `https://github.com/pypa/package.git#subdirectory=pkg_a` and
/// `https://github.com/pypa/package.git#subdirectory=pkg_b` would map to different
/// [`CanonicalUrl`] values, but the same [`RepositoryUrl`], since they map to the same
/// resource.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub struct RepositoryUrl(Url);

impl RepositoryUrl {
    pub fn new(url: &Url) -> Self {
        let mut url = CanonicalUrl::new(url).0;

        // If a Git URL ends in a reference (like a branch, tag, or commit), remove it.
        let mut url = if url.scheme().starts_with("git+") {
            if let Some(prefix) = url
                .path()
                .rsplit_once('@')
                .map(|(prefix, _suffix)| prefix.to_string())
            {
                url.set_path(&prefix);
            }

            // Remove the `git+` prefix.
            let url_as_str = &url.as_str()[4..];
            Url::parse(url_as_str).expect("url should be valid")
        } else {
            url
        };

        // Drop any fragments and query parameters.
        url.set_fragment(None);
        url.set_query(None);

        Self(url)
    }

    pub fn parse(url: &str) -> Result<Self, url::ParseError> {
        Ok(Self::new(&Url::parse(url)?))
    }

    /// Return the underlying [`Url`] of this repository.
    pub fn into_url(self) -> Url {
        self.into()
    }
}

/// Remove the credentials from a URL, allowing the generic `git` username (without a password)
/// in SSH URLs, as in, `ssh://git@github.com/...`.
pub fn redact_credentials(url: &mut Url) {
    // For URLs that use the `git` convention (i.e., `ssh://git@github.com/...`), avoid dropping the
    // username.
    if url.scheme() == "ssh" && url.username() == "git" && url.password().is_none() {
        return;
    }
    let _ = url.set_password(None);
    let _ = url.set_username("");
}

impl From<RepositoryUrl> for Url {
    fn from(url: RepositoryUrl) -> Self {
        url.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_credential_does_not_affect_canonical_url() -> Result<(), url::ParseError> {
        let url_without_creds =
            CanonicalUrl::parse("https://example.com/pypa/sample-namespace-packages.git@2.0.0")?;

        let url_with_creds = CanonicalUrl::parse(
            "https://user:foo@example.com/pypa/sample-namespace-packages.git@2.0.0",
        )?;
        assert_eq!(
            url_without_creds, url_with_creds,
            "URLs with no user credentials should be the same as URLs with different user credentials",
        );

        let url_with_only_password = CanonicalUrl::parse(
            "https://:bar@example.com/pypa/sample-namespace-packages.git@2.0.0",
        )?;
        assert_eq!(
            url_with_creds, url_with_only_password,
            "URLs with no username, though with a password, should be the same as URLs with different user credentials",
        );

        let url_with_username = CanonicalUrl::parse(
            "https://user:@example.com/pypa/sample-namespace-packages.git@2.0.0",
        )?;
        assert_eq!(
            url_with_creds, url_with_username,
            "URLs with no password, though with a username, should be the same as URLs with different user credentials",
        );

        Ok(())
    }

    #[test]
    fn canonical_url() -> Result<(), url::ParseError> {
        // Two URLs should be considered equal regardless of the `.git` suffix.
        assert_eq!(
            CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git")?,
            CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages")?,
        );

        // Two URLs should be considered equal regardless of the `.git` suffix.
        assert_eq!(
            CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git@2.0.0")?,
            CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages@2.0.0")?,
        );

        // Two URLs should be _not_ considered equal if they point to different repositories.
        assert_ne!(
            CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git")?,
            CanonicalUrl::parse("git+https://github.com/pypa/sample-packages.git")?,
        );

        // Two URLs should _not_ be considered equal if they request different subdirectories.
        assert_ne!(
             CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git#subdirectory=pkg_resources/pkg_a")?,
             CanonicalUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git#subdirectory=pkg_resources/pkg_b")?,
         );

        // Two URLs should _not_ be considered equal if they request different commit tags.
        assert_ne!(
            CanonicalUrl::parse(
                "git+https://github.com/pypa/sample-namespace-packages.git@v1.0.0"
            )?,
            CanonicalUrl::parse(
                "git+https://github.com/pypa/sample-namespace-packages.git@v2.0.0"
            )?,
        );

        // Two URLs that cannot be a base should be considered equal.
        assert_eq!(
            CanonicalUrl::parse("git+https:://github.com/pypa/sample-namespace-packages.git")?,
            CanonicalUrl::parse("git+https:://github.com/pypa/sample-namespace-packages.git")?,
        );

        Ok(())
    }

    #[test]
    fn repository_url() -> Result<(), url::ParseError> {
        // Two URLs should be considered equal regardless of the `.git` suffix.
        assert_eq!(
            RepositoryUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git")?,
            RepositoryUrl::parse("git+https://github.com/pypa/sample-namespace-packages")?,
        );

        // Two URLs should be considered equal regardless of the `.git` suffix.
        assert_eq!(
            RepositoryUrl::parse(
                "git+https://github.com/pypa/sample-namespace-packages.git@2.0.0"
            )?,
            RepositoryUrl::parse("git+https://github.com/pypa/sample-namespace-packages@2.0.0")?,
        );

        // Two URLs should be _not_ considered equal if they point to different repositories.
        assert_ne!(
            RepositoryUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git")?,
            RepositoryUrl::parse("git+https://github.com/pypa/sample-packages.git")?,
        );

        // Two URLs should be considered equal if they map to the same repository, even if they
        // request different subdirectories.
        assert_eq!(
             RepositoryUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git#subdirectory=pkg_resources/pkg_a")?,
             RepositoryUrl::parse("git+https://github.com/pypa/sample-namespace-packages.git#subdirectory=pkg_resources/pkg_b")?,
         );

        // Two URLs should be considered equal if they map to the same repository, even if they
        // request different commit tags.
        assert_eq!(
            RepositoryUrl::parse(
                "git+https://github.com/pypa/sample-namespace-packages.git@v1.0.0"
            )?,
            RepositoryUrl::parse(
                "git+https://github.com/pypa/sample-namespace-packages.git@v2.0.0"
            )?,
        );

        Ok(())
    }
}
