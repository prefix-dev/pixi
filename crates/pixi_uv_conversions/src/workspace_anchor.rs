use std::path::{Path, PathBuf};

use typed_path::Utf8TypedPathBuf;
use uv_pep508::VerbatimUrl;

#[derive(Debug, thiserror::Error)]
pub enum AnchorError {
    #[error("expected given path for {0} but none found")]
    NoGivenPath(String),
    #[error("cannot make {0} relative to {1}")]
    CannotMakeRelative(String, String),
    #[error("path is not UTF-8: {}", .0.display())]
    NotUtf8(PathBuf),
}

/// Converts absolute paths to workspace-root-relative lockfile paths.
///
/// Carries the workspace root so that relative paths never lose their anchor
/// as they move through conversion code.
pub struct WorkspaceAnchor<'a> {
    root: &'a Path,
}

impl<'a> WorkspaceAnchor<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }

    /// Lockfile-relative form of `abs_path`. Prepends `./` when the result
    /// descends into the workspace (legacy lockfile convention); preserves
    /// `..` ascents as-is. On Windows, backslashes are replaced with `/`.
    pub fn relative_path(&self, abs_path: &Path) -> Result<Utf8TypedPathBuf, AnchorError> {
        let rel = pathdiff::diff_paths(abs_path, self.root).ok_or_else(|| {
            AnchorError::CannotMakeRelative(
                abs_path.to_string_lossy().to_string(),
                self.root.to_string_lossy().to_string(),
            )
        })?;

        let std_path = if !rel.starts_with("..") {
            PathBuf::from(".").join(&rel)
        } else {
            rel
        };

        let path_str = std_path
            .to_str()
            .ok_or_else(|| AnchorError::NotUtf8(std_path.clone()))?;

        Ok(if cfg!(windows) {
            Utf8TypedPathBuf::from(path_str.replace('\\', "/"))
        } else {
            Utf8TypedPathBuf::from(path_str)
        })
    }

    /// Workspace-relative `given` for a `file://` URL, or `None` if the URL
    /// is non-file or cannot be relativized.
    ///
    /// If the original `given` was an explicit absolute path (not a `file://` URL), it is
    /// preserved, mirroring the behavior of [`WorkspaceAnchor::given_for_location`].
    pub fn relative_given_for_file_url(&self, url: &VerbatimUrl) -> Option<String> {
        let url_ref: &url::Url = url;
        if url_ref.scheme() != "file" {
            return None;
        }
        // Preserve absolute paths the user explicitly wrote.
        if let Some(given) = url.given() {
            if !given.starts_with("file://") && PathBuf::from(given).is_absolute() {
                return Some(given.to_owned());
            }
        }
        let abs_path = url_ref.to_file_path().ok()?;
        self.relative_path(&abs_path).ok().map(|p| p.to_string())
    }

    /// Lockfile path for a uv path/directory `location`, honoring
    /// "keep absolute if the user wrote it absolute" semantics.
    ///
    /// - `given` starts with `file://` → always relativize (PEP 508 path deps
    ///   produce `file://` givens; the intent is relative).
    /// - `given` is an absolute bare path → preserve as-is.
    /// - `given` is relative → re-anchor to workspace root via `install_path`.
    pub fn given_for_location(
        &self,
        url: &VerbatimUrl,
        install_path: &Path,
    ) -> Result<Utf8TypedPathBuf, AnchorError> {
        let given = url
            .given()
            .ok_or_else(|| AnchorError::NoGivenPath(url.to_string()))?;

        let keep_abs = if given.starts_with("file://") {
            false
        } else {
            PathBuf::from(given).is_absolute()
        };

        if keep_abs {
            let path_str = install_path
                .to_str()
                .ok_or_else(|| AnchorError::NotUtf8(install_path.to_path_buf()))?;
            Ok(if cfg!(windows) {
                Utf8TypedPathBuf::from(path_str.replace('\\', "/"))
            } else {
                Utf8TypedPathBuf::from(path_str)
            })
        } else {
            self.relative_path(install_path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn relative_path_descending_gets_dot_slash() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let result = anchor.relative_path(Path::new("/workspace/pkg-a")).unwrap();
        assert_eq!(result.as_str(), "./pkg-a");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn relative_path_ascending_keeps_dotdot() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let result = anchor.relative_path(Path::new("/other/pkg")).unwrap();
        assert_eq!(result.as_str(), "../other/pkg");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn relative_given_for_file_url_returns_none_for_non_file() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let url = uv_pep508::VerbatimUrl::parse_url("https://example.com/pkg.whl")
            .unwrap()
            .with_given("https://example.com/pkg.whl");
        assert!(anchor.relative_given_for_file_url(&url).is_none());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn relative_given_for_file_url_preserves_absolute_given() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let url = uv_pep508::VerbatimUrl::parse_url("file:///abs/pkg")
            .unwrap()
            .with_given("/abs/pkg");
        assert_eq!(
            anchor.relative_given_for_file_url(&url),
            Some("/abs/pkg".to_owned())
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn relative_given_for_file_url_reanchors_file_scheme() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let url = uv_pep508::VerbatimUrl::parse_url("file:///workspace/pkg-b")
            .unwrap()
            .with_given("file:///workspace/pkg-b");
        assert_eq!(
            anchor.relative_given_for_file_url(&url),
            Some("./pkg-b".to_owned())
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn given_for_location_relativizes_file_scheme_given() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let url = uv_pep508::VerbatimUrl::parse_url("file:///workspace/pkg-b")
            .unwrap()
            .with_given("file:///workspace/pkg-b");
        let result = anchor
            .given_for_location(&url, Path::new("/workspace/pkg-b"))
            .unwrap();
        assert_eq!(result.as_str(), "./pkg-b");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn given_for_location_preserves_absolute_given() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let url = uv_pep508::VerbatimUrl::parse_url("file:///abs/pkg")
            .unwrap()
            .with_given("/abs/pkg");
        let result = anchor
            .given_for_location(&url, Path::new("/abs/pkg"))
            .unwrap();
        assert_eq!(result.as_str(), "/abs/pkg");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn given_for_location_relativizes_relative_given() {
        let anchor = WorkspaceAnchor::new(Path::new("/workspace"));
        let url = uv_pep508::VerbatimUrl::parse_url("file:///workspace/pkg-b")
            .unwrap()
            .with_given("./pkg-b");
        let result = anchor
            .given_for_location(&url, Path::new("/workspace/pkg-b"))
            .unwrap();
        assert_eq!(result.as_str(), "./pkg-b");
    }
}
