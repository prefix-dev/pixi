//! Git repository fixture for testing git-based dependencies without network access.
//!
//! This module provides [`GitRepoFixture`] which creates temporary git repositories
//! from versioned fixture directories for testing purposes.

use std::{collections::HashMap, path::Path};

use tempfile::TempDir;

use super::cargo_workspace_dir;

/// A temporary git repository created from fixture directories.
///
/// Each subdirectory in the fixture (named like `001_commit-message`) becomes
/// a commit in the repository. Directories are processed in sorted order,
/// allowing you to build up a git history from versioned snapshots.
///
/// If a commit message starts with `v` (e.g., `v0.1.0`), a git tag is created
/// for that commit.
///
/// # Example fixture structure
///
/// ```text
/// tests/data/git-fixtures/minimal-pypi-package/
/// ├── 001_v0.1.0/
/// │   ├── pyproject.toml
/// │   └── src/minimal_package/__init__.py
/// └── 002_v0.2.0/
///     ├── pyproject.toml
///     └── src/minimal_package/__init__.py
/// ```
///
/// # Example usage
///
/// ```ignore
/// let fixture = GitRepoFixture::new("minimal-pypi-package");
/// // fixture.url is a git+file:// URL
/// // fixture.commits[0] is the first commit hash
/// // fixture.commits[1] is the second commit hash
/// // fixture.tags["v0.1.0"] is the commit hash for that tag
/// ```
pub struct GitRepoFixture {
    /// Temporary directory containing the git repository.
    /// Kept alive to prevent cleanup until the fixture is dropped.
    _tempdir: TempDir,

    /// Git URL for the repository (git+file://...).
    pub url: String,

    /// Base URL for the repository (file://... without git+ prefix).
    pub base_url: url::Url,

    /// SHA hashes of all commits in order (first commit is index 0).
    pub commits: Vec<String>,

    /// Map of tag names to commit hashes.
    pub tags: HashMap<String, String>,
}

impl GitRepoFixture {
    /// Creates a git repository from numbered fixture directories.
    ///
    /// Looks for directories in `tests/data/git-fixtures/{fixture_name}/` with names
    /// like `001_commit-message`, `002_commit-message`, etc. Each directory's contents
    /// are copied to the repo and committed in sorted order.
    ///
    /// The commit message is extracted from the directory name (the part after `_`).
    /// If the commit message starts with `v`, a git tag is created with that name.
    pub fn new(fixture_name: &str) -> Self {
        let tempdir = TempDir::new().expect("failed to create temp dir");
        let repo_path = tempdir.path().join(fixture_name);
        fs_err::create_dir_all(&repo_path).expect("failed to create repo dir");

        let fixture_base = cargo_workspace_dir()
            .join("tests/data/git-fixtures")
            .join(fixture_name);

        // Initialize git repo
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&repo_path)
            .output()
            .expect("failed to init git repo");

        // Configure git user for commits
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&repo_path)
            .output()
            .expect("failed to configure git email");
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo_path)
            .output()
            .expect("failed to configure git name");

        // Get commit directories sorted by name
        let mut commit_dirs: Vec<_> = fs_err::read_dir(&fixture_base)
            .expect("failed to read fixture dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        commit_dirs.sort_by_key(|e| e.file_name());

        let mut commits = Vec::new();
        let mut tags = HashMap::new();

        for entry in commit_dirs {
            let dir_name = entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();

            // Extract commit message from directory name (after the number prefix)
            let commit_msg = dir_name_str.split_once('_').map(|(_, msg)| msg).unwrap();

            copy_dir_contents(&entry.path(), &repo_path);

            std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(&repo_path)
                .output()
                .expect("failed to git add");
            std::process::Command::new("git")
                .args(["commit", "--message", commit_msg])
                .current_dir(&repo_path)
                .output()
                .expect("failed to git commit");

            let commit_hash = String::from_utf8(
                std::process::Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .current_dir(&repo_path)
                    .output()
                    .expect("failed to get commit hash")
                    .stdout,
            )
            .expect("invalid utf8")
            .trim()
            .to_string();

            // Create a git tag if the commit message starts with 'v'
            if commit_msg.starts_with('v') {
                std::process::Command::new("git")
                    .args(["tag", commit_msg])
                    .current_dir(&repo_path)
                    .output()
                    .expect("failed to create git tag");
                tags.insert(commit_msg.to_string(), commit_hash.clone());
            }

            commits.push(commit_hash);
        }

        let base_url =
            url::Url::from_directory_path(&repo_path).expect("failed to create URL from repo path");

        Self {
            _tempdir: tempdir,
            url: format!("git+{base_url}"),
            base_url,
            commits,
            tags,
        }
    }

    /// Returns the first commit hash, or panics if there are no commits.
    pub fn first_commit(&self) -> &str {
        self.commits.first().expect("no commits in fixture")
    }

    /// Returns the latest (most recent) commit hash, or panics if there are no commits.
    pub fn latest_commit(&self) -> &str {
        self.commits.last().expect("no commits in fixture")
    }

    /// Returns the commit hash for a given tag name.
    pub fn tag_commit(&self, tag: &str) -> &str {
        self.tags
            .get(tag)
            .unwrap_or_else(|| panic!("tag '{tag}' not found in fixture"))
    }
}

/// Recursively copy directory contents from src to dst.
fn copy_dir_contents(src: &Path, dst: &Path) {
    for entry in fs_err::read_dir(src).expect("failed to read fixture dir") {
        let entry = entry.expect("failed to read dir entry");
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            fs_err::create_dir_all(&dst_path).expect("failed to create dir");
            copy_dir_contents(&src_path, &dst_path);
        } else {
            fs_err::copy(&src_path, &dst_path).expect("failed to copy file");
        }
    }
}
