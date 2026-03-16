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

impl std::str::FromStr for GitUrlWithPrefix {
    type Err = url::ParseError;

    /// Parse a URL that may have git+ prefix
    fn from_str(url_str: &str) -> Result<Self, Self::Err> {
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
}

impl From<Url> for GitUrlWithPrefix {
    /// Create from a base URL
    fn from(base_url: Url) -> Self {
        let url_str = base_url.as_str();
        // This should not fail since we're parsing a valid URL
        Self::from_str(url_str).expect("Valid URL should parse successfully")
    }
}

impl From<&Url> for GitUrlWithPrefix {
    /// Create from a base URL
    fn from(base_url: &Url) -> Self {
        let url_str = base_url.as_str();
        // This should not fail since we're parsing a valid URL
        Self::from_str(url_str).expect("Valid URL should parse successfully")
    }
}

impl GitUrlWithPrefix {
    /// Get the URL without git+ prefix (for GitUrl creation)
    pub fn without_git_prefix(&self) -> &Url {
        &self.base_url
    }

    /// Get the URL with git+ prefix if it originally had one (for VerbatimUrl creation)
    pub fn with_git_prefix(&self) -> String {
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
        let display_safe_url = DisplaySafeUrl::parse(&self.with_git_prefix())?;
        Ok(VerbatimUrl::from_url(display_safe_url))
    }

    /// Check if this URL has a git+ prefix
    pub fn has_git_plus_prefix(&self) -> bool {
        self.has_git_plus
    }
}

impl std::fmt::Display for GitUrlWithPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.with_git_prefix())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_with_git_plus() {
        let url = GitUrlWithPrefix::from_str("git+https://github.com/user/repo.git").unwrap();
        assert!(url.has_git_plus_prefix());
        assert_eq!(
            url.without_git_prefix().to_string(),
            "https://github.com/user/repo.git"
        );
        assert_eq!(
            url.with_git_prefix(),
            "git+https://github.com/user/repo.git"
        );
    }

    #[test]
    fn test_parse_without_git_plus() {
        let url = GitUrlWithPrefix::from_str("https://github.com/user/repo.git").unwrap();
        assert!(!url.has_git_plus_prefix());
        assert_eq!(
            url.without_git_prefix().to_string(),
            "https://github.com/user/repo.git"
        );
        assert_eq!(url.with_git_prefix(), "https://github.com/user/repo.git");
    }

    #[test]
    fn test_ssh_url_with_git_plus() {
        let url = GitUrlWithPrefix::from_str("git+ssh://git@github.com/user/repo.git").unwrap();
        assert!(url.has_git_plus_prefix());
        assert_eq!(
            url.without_git_prefix().to_string(),
            "ssh://git@github.com/user/repo.git"
        );
        assert_eq!(
            url.with_git_prefix(),
            "git+ssh://git@github.com/user/repo.git"
        );
    }

    #[test]
    fn test_from_url() {
        let base_url = Url::from_str("https://github.com/user/repo.git").unwrap();
        let git_url = GitUrlWithPrefix::from(&base_url);
        assert!(!git_url.has_git_plus_prefix());
        assert_eq!(git_url.without_git_prefix(), &base_url);
    }

    #[test]
    fn test_to_verbatim_url() {
        let url = GitUrlWithPrefix::from_str("git+https://github.com/user/repo.git").unwrap();
        let verbatim = url.to_verbatim_url().unwrap();
        assert_eq!(verbatim.to_string(), "git+https://github.com/user/repo.git");
    }

    #[test]
    fn test_display_safe_url() {
        let url = GitUrlWithPrefix::from_str("git+https://github.com/user/repo.git").unwrap();
        let display_safe = url.to_display_safe_url();
        assert_eq!(display_safe.to_string(), "https://github.com/user/repo.git");
    }
}
