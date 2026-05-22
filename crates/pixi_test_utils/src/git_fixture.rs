//! Git repository fixture for testing git-based dependencies without network access.
//!
//! This module provides [`GitRepoFixture`] which creates temporary git repositories
//! from versioned fixture directories for testing purposes.

use std::{collections::HashMap, path::Path};

use tempfile::TempDir;

/// True if `path` looks like a gitattributes file (real or `dot-gitattributes`
/// placeholder) with a non-commented `filter=lfs` line.
fn gitattributes_uses_lfs(path: &Path) -> bool {
    let Ok(contents) = fs_err::read_to_string(path) else {
        return false;
    };
    contents.lines().any(|line| {
        let line = line.trim();
        !line.starts_with('#') && line.contains("filter=lfs")
    })
}

/// File name a fixture uses in place of `.gitattributes` so the outer repo
/// (this one) doesn't interpret the LFS filter rules. Renamed back to
/// `.gitattributes` when copied into the fixture's working repo.
const GITATTRIBUTES_PLACEHOLDER: &str = "dot-gitattributes";

/// Returns the in-repo name we should copy `entry` to: `.gitattributes` if
/// the source is a `dot-gitattributes` placeholder, otherwise unchanged.
fn dest_name(name: &std::ffi::OsStr) -> std::ffi::OsString {
    if name == std::ffi::OsStr::new(GITATTRIBUTES_PLACEHOLDER) {
        std::ffi::OsString::from(".gitattributes")
    } else {
        name.to_owned()
    }
}

/// True if any gitattributes file under `dir` configures the lfs filter.
fn dir_uses_lfs(dir: &Path) -> bool {
    let Ok(entries) = fs_err::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_uses_lfs(&path) {
                return true;
            }
        } else {
            let name = path.file_name();
            let is_attrs = name == Some(std::ffi::OsStr::new(".gitattributes"))
                || name == Some(std::ffi::OsStr::new(GITATTRIBUTES_PLACEHOLDER));
            if is_attrs && gitattributes_uses_lfs(&path) {
                return true;
            }
        }
    }
    false
}

/// True if `git lfs version` succeeds.
fn git_lfs_available() -> bool {
    std::process::Command::new("git")
        .args(["lfs", "version"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Returns the path to the Cargo workspace root.
fn cargo_workspace_dir() -> &'static Path {
    Path::new(env!("CARGO_WORKSPACE_DIR"))
}

/// A temporary git repository created from fixture directories.
///
/// Each subdirectory in the fixture (named like `001_commit-message`) becomes
/// a commit in the repository. Directories are processed in sorted order,
/// allowing you to build up a git history from versioned snapshots.
///
/// If a commit message starts with `v` (e.g., `v0.1.0`), a git tag is created
/// for that commit.
///
/// # Git LFS
///
/// If any commit dir has a `.gitattributes` (or the `dot-gitattributes`
/// placeholder — see below) with `filter=lfs`, the fixture runs
/// `git lfs install --local` before commits so matching files are stored
/// as LFS pointers with blobs in `.git/lfs/objects/`. Panics if `git-lfs`
/// is not installed.
///
/// **`dot-gitattributes` convention**: a file named `dot-gitattributes`
/// in a commit dir is renamed to `.gitattributes` when copied into the
/// fixture repo. This lets you ship LFS-flavoured attribute rules in
/// fixtures without having the outer (this) repo apply them too.
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

    /// Path to the repository working directory. Useful for tests that
    /// need to mutate the repo after construction (extra branches, more
    /// commits, etc.).
    pub repo_path: std::path::PathBuf,

    /// Git URL for the repository (git+file://...).
    pub url: String,

    /// Base URL for the repository (file://... without git+ prefix).
    pub base_url: url::Url,

    /// SHA hashes of all commits in order (first commit is index 0).
    pub commits: Vec<String>,

    /// Map of tag names to commit hashes.
    pub tags: HashMap<String, String>,

    /// True if `git lfs install --local` was run for this fixture.
    pub uses_lfs: bool,
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
        let fixture_base = cargo_workspace_dir()
            .join("tests/data/git-fixtures")
            .join(fixture_name);
        Self::from_path(&fixture_base, fixture_name)
    }

    /// Creates a git repository from a specific fixture path.
    ///
    /// This allows using fixture directories from any location, not just the
    /// default `tests/data/git-fixtures` directory.
    pub fn from_path(fixture_base: &Path, repo_name: &str) -> Self {
        let tempdir = TempDir::new().expect("failed to create temp dir");
        let repo_path = tempdir.path().join(repo_name);
        fs_err::create_dir_all(&repo_path).expect("failed to create repo dir");

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
        // Defeat any global commit/tag signing config so the fixture is
        // self-contained on hosts that mandate signed commits.
        for key in ["commit.gpgsign", "tag.gpgsign"] {
            std::process::Command::new("git")
                .args(["config", key, "false"])
                .current_dir(&repo_path)
                .output()
                .unwrap_or_else(|err| panic!("failed to unset {key}: {err}"));
        }

        // Get commit directories sorted by name
        let mut commit_dirs: Vec<_> = fs_err::read_dir(fixture_base)
            .expect("failed to read fixture dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        commit_dirs.sort_by_key(|e| e.file_name());

        // Auto-detect LFS via `filter=lfs` in any fixture .gitattributes.
        let uses_lfs = commit_dirs.iter().any(|d| dir_uses_lfs(&d.path()));
        if uses_lfs {
            assert!(
                git_lfs_available(),
                "git-lfs is required for fixture '{repo_name}' (its .gitattributes uses filter=lfs) \
                 but `git lfs version` failed. Install git-lfs to run this test."
            );
            std::process::Command::new("git")
                .args(["lfs", "install", "--local"])
                .current_dir(&repo_path)
                .output()
                .expect("failed to run `git lfs install --local`");
        }

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
            repo_path,
            url: format!("git+{base_url}"),
            base_url,
            commits,
            tags,
            uses_lfs,
        }
    }

    /// Run a `git` command against the fixture's working directory and
    /// return trimmed stdout. Panics on non-zero exit. Useful for tests
    /// that need to extend the repo after construction (e.g., creating
    /// extra branches that the numbered-fixture format can't express).
    pub fn git(&self, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .unwrap_or_else(|err| panic!("failed to spawn `git {}`: {err}", args.join(" ")));
        assert!(
            output.status.success(),
            "`git {}` failed: stdout={:?} stderr={:?}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout)
            .expect("git output must be utf-8")
            .trim()
            .to_string()
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

/// Recursively copy directory contents from src to dst, renaming
/// `dot-gitattributes` to `.gitattributes` along the way.
fn copy_dir_contents(src: &Path, dst: &Path) {
    for entry in fs_err::read_dir(src).expect("failed to read fixture dir") {
        let entry = entry.expect("failed to read dir entry");
        let src_path = entry.path();
        let dst_path = dst.join(dest_name(&entry.file_name()));

        if src_path.is_dir() {
            fs_err::create_dir_all(&dst_path).expect("failed to create dir");
            copy_dir_contents(&src_path, &dst_path);
        } else {
            fs_err::copy(&src_path, &dst_path).expect("failed to copy file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lfs_filter() {
        let tmp = TempDir::new().unwrap();
        fs_err::write(
            tmp.path().join(".gitattributes"),
            "*.bin filter=lfs diff=lfs merge=lfs -text\n",
        )
        .unwrap();
        assert!(dir_uses_lfs(tmp.path()));
    }

    #[test]
    fn ignores_commented_lfs_lines() {
        let tmp = TempDir::new().unwrap();
        fs_err::write(
            tmp.path().join(".gitattributes"),
            "# *.bin filter=lfs diff=lfs merge=lfs -text\n*.txt text\n",
        )
        .unwrap();
        assert!(!dir_uses_lfs(tmp.path()));
    }

    #[test]
    fn ignores_non_lfs_gitattributes() {
        let tmp = TempDir::new().unwrap();
        fs_err::write(tmp.path().join(".gitattributes"), "*.txt text\n").unwrap();
        assert!(!dir_uses_lfs(tmp.path()));
    }

    #[test]
    fn detects_lfs_in_nested_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b");
        fs_err::create_dir_all(&nested).unwrap();
        fs_err::write(nested.join(".gitattributes"), "data/* filter=lfs\n").unwrap();
        assert!(dir_uses_lfs(tmp.path()));
    }
}
