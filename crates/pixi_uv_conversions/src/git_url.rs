use std::str::FromStr;
use url::Url;
use uv_pep508::VerbatimUrl;
use uv_redacted::DisplaySafeUrl;

/// A URL that may have a git+ prefix, with methods to handle both representations
#[derive(Debug, Clone, PartialEq)]
pub struct GitUrlWithPrefix {
    /// The base URL without git+ prefix
    base_url: Url,
    /// Whether this URL originally had a git+ prefix
    has_git_plus: bool,
}

impl GitUrlWithPrefix {
    /// Parse a URL that may have git+ prefix
    pub fn parse(url_str: &str) -> Result<Self, url::ParseError> {
        let (base_url_str, has_git_plus) = if let Some(stripped) = url_str.strip_prefix("git+") {
            (stripped, true)
        } else {
            (url_str, false)
        };

        let base_url = Url::parse(base_url_str)?;
        Ok(Self {
            base_url,
            has_git_plus,
        })
    }

    /// Create from a URL, detecting git+ prefix automatically
    pub fn from_url(url: &Url) -> Self {
        let url_str = url.to_string();
        // This should not fail since we're parsing a valid URL
        Self::parse(&url_str).expect("Valid URL should parse successfully")
    }

    /// Get the URL without git+ prefix (for GitUrl creation)
    pub fn without_git_plus(&self) -> &Url {
        &self.base_url
    }

    /// Get the URL with git+ prefix if it originally had one (for VerbatimUrl creation)
    pub fn with_git_plus(&self) -> String {
        if self.has_git_plus {
            format!("git+{}", self.base_url)
        } else {
            self.base_url.to_string()
        }
    }

    /// Convert to DisplaySafeUrl without git+ prefix
    pub fn to_display_safe_url(&self) -> DisplaySafeUrl {
        self.base_url.clone().into()
    }

    /// Convert to VerbatimUrl (preserving git+ prefix)
    pub fn to_verbatim_url(&self) -> Result<VerbatimUrl, url::ParseError> {
        let display_safe_url = DisplaySafeUrl::parse(&self.with_git_plus())?;
        Ok(VerbatimUrl::from_url(display_safe_url))
    }

    /// Get the base URL as a DisplaySafeUrl (without git+ prefix)
    pub fn as_display_safe_url(&self) -> DisplaySafeUrl {
        self.base_url.clone().into()
    }

    /// Check if this URL has a git+ prefix
    pub fn has_git_plus_prefix(&self) -> bool {
        self.has_git_plus
    }
}

impl FromStr for GitUrlWithPrefix {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl std::fmt::Display for GitUrlWithPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.with_git_plus())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_with_git_plus() {
        let url = GitUrlWithPrefix::parse("git+https://github.com/user/repo.git").unwrap();
        assert!(url.has_git_plus_prefix());
        assert_eq!(
            url.without_git_plus().to_string(),
            "https://github.com/user/repo.git"
        );
        assert_eq!(url.with_git_plus(), "git+https://github.com/user/repo.git");
    }

    #[test]
    fn test_parse_without_git_plus() {
        let url = GitUrlWithPrefix::parse("https://github.com/user/repo.git").unwrap();
        assert!(!url.has_git_plus_prefix());
        assert_eq!(
            url.without_git_plus().to_string(),
            "https://github.com/user/repo.git"
        );
        assert_eq!(url.with_git_plus(), "https://github.com/user/repo.git");
    }

    #[test]
    fn test_ssh_url_with_git_plus() {
        let url = GitUrlWithPrefix::parse("git+ssh://git@github.com/user/repo.git").unwrap();
        assert!(url.has_git_plus_prefix());
        assert_eq!(
            url.without_git_plus().to_string(),
            "ssh://git@github.com/user/repo.git"
        );
        assert_eq!(
            url.with_git_plus(),
            "git+ssh://git@github.com/user/repo.git"
        );
    }

    #[test]
    fn test_from_url() {
        let base_url = Url::parse("https://github.com/user/repo.git").unwrap();
        let git_url = GitUrlWithPrefix::from_url(&base_url);
        assert!(!git_url.has_git_plus_prefix());
        assert_eq!(git_url.without_git_plus(), &base_url);
    }

    #[test]
    fn test_to_verbatim_url() {
        let url = GitUrlWithPrefix::parse("git+https://github.com/user/repo.git").unwrap();
        let verbatim = url.to_verbatim_url().unwrap();
        assert_eq!(verbatim.to_string(), "git+https://github.com/user/repo.git");
    }

    #[test]
    fn test_display_safe_url() {
        let url = GitUrlWithPrefix::parse("git+https://github.com/user/repo.git").unwrap();
        let display_safe = url.to_display_safe_url();
        assert_eq!(display_safe.to_string(), "https://github.com/user/repo.git");
    }
}
