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
        if url
            .given()
            .is_some_and(|g| !g.starts_with("file://") && PathBuf::from(g).is_absolute())
        {
            return url.given().map(str::to_owned);
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

    #[test]
    fn relative_path_descending_gets_dot_slash() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_a = tmp.path().join("pkg-a");
        fs_err::create_dir_all(&pkg_a).unwrap();
        let anchor = WorkspaceAnchor::new(tmp.path());
        let result = anchor.relative_path(&pkg_a).unwrap();
        assert_eq!(result.as_str(), "./pkg-a");
    }

    #[test]
    fn relative_path_ascending_keeps_dotdot() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let other_pkg = tmp.path().join("other").join("pkg");
        fs_err::create_dir_all(&workspace).unwrap();
        fs_err::create_dir_all(&other_pkg).unwrap();
        let anchor = WorkspaceAnchor::new(&workspace);
        let result = anchor.relative_path(&other_pkg).unwrap();
        assert_eq!(result.as_str(), "../other/pkg");
    }

    #[test]
    fn relative_given_for_file_url_returns_none_for_non_file() {
        let tmp = tempfile::tempdir().unwrap();
        let anchor = WorkspaceAnchor::new(tmp.path());
        let url = uv_pep508::VerbatimUrl::parse_url("https://example.com/pkg.whl")
            .unwrap()
            .with_given("https://example.com/pkg.whl");
        assert!(anchor.relative_given_for_file_url(&url).is_none());
    }

    #[test]
    fn relative_given_for_file_url_preserves_absolute_given() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let abs_pkg = tmp.path().join("abs").join("pkg");
        fs_err::create_dir_all(&abs_pkg).unwrap();
        let abs_given = abs_pkg.to_str().unwrap().to_owned();
        let url = uv_pep508::VerbatimUrl::from_absolute_path(&abs_pkg)
            .unwrap()
            .with_given(abs_given.as_str());
        let anchor = WorkspaceAnchor::new(&workspace);
        assert_eq!(anchor.relative_given_for_file_url(&url), Some(abs_given));
    }

    #[test]
    fn relative_given_for_file_url_reanchors_file_scheme() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_b = tmp.path().join("pkg-b");
        fs_err::create_dir_all(&pkg_b).unwrap();
        let file_url = uv_pep508::VerbatimUrl::from_absolute_path(&pkg_b).unwrap();
        let file_url_string = file_url.to_url().to_string();
        let url = file_url.with_given(file_url_string.as_str());
        let anchor = WorkspaceAnchor::new(tmp.path());
        assert_eq!(
            anchor.relative_given_for_file_url(&url),
            Some("./pkg-b".to_owned())
        );
    }

    #[test]
    fn given_for_location_relativizes_file_scheme_given() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_b = tmp.path().join("pkg-b");
        fs_err::create_dir_all(&pkg_b).unwrap();
        let file_url = uv_pep508::VerbatimUrl::from_absolute_path(&pkg_b).unwrap();
        let file_url_string = file_url.to_url().to_string();
        let url = file_url.with_given(file_url_string.as_str());
        let anchor = WorkspaceAnchor::new(tmp.path());
        let result = anchor.given_for_location(&url, &pkg_b).unwrap();
        assert_eq!(result.as_str(), "./pkg-b");
    }

    #[test]
    fn given_for_location_preserves_absolute_given() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let abs_pkg = tmp.path().join("abs").join("pkg");
        fs_err::create_dir_all(&abs_pkg).unwrap();
        let url = uv_pep508::VerbatimUrl::from_absolute_path(&abs_pkg)
            .unwrap()
            .with_given(abs_pkg.to_str().unwrap());
        let anchor = WorkspaceAnchor::new(&workspace);
        let result = anchor.given_for_location(&url, &abs_pkg).unwrap();
        // given_for_location normalizes backslashes to forward slashes
        let expected = abs_pkg.to_str().unwrap().replace('\\', "/");
        assert_eq!(result.as_str(), expected.as_str());
    }

    #[test]
    fn given_for_location_relativizes_relative_given() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_b = tmp.path().join("pkg-b");
        fs_err::create_dir_all(&pkg_b).unwrap();
        let url = uv_pep508::VerbatimUrl::from_absolute_path(&pkg_b)
            .unwrap()
            .with_given("./pkg-b");
        let anchor = WorkspaceAnchor::new(tmp.path());
        let result = anchor.given_for_location(&url, &pkg_b).unwrap();
        assert_eq!(result.as_str(), "./pkg-b");
    }
}
