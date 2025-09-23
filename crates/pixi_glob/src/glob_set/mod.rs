//! Convenience wrapper around `ignore` that emulates the glob semantics pixi expects.
//!
//! Notable behavioural tweaks compared to vanilla gitignore parsing, so that it behaves more like unix globbing with special rules:
//! - Globs are rebased to a shared search root so patterns like `../src/*.rs` keep working even
//!   when the caller starts from a nested directory.
//! - Negated patterns that start with `**/` are treated as global exclusions. We skip rebasing
//!   those so `!**/build.rs` still hides every `build.rs`, regardless of the effective root.
//! - Plain file names without meta characters (e.g. `pixi.toml`) are anchored to the search root
//!   instead of matching anywhere below it. This mirrors the behaviour we had with the previous
//!   wax-based implementation.
//! - Negated literals (e.g. `!pixi.toml`) are anchored the same way, which lets recipes ignore a
//!   single file at the root without accidentally hiding copies deeper in the tree.

mod glob_walk_root;
mod walk;

use std::path::{Path, PathBuf};

use thiserror::Error;

use glob_walk_root::{GlobWalkRoot, WalkRootsError};

/// A glob set implemented using the `ignore` crate (globset + fast walker).
pub struct GlobSet {
    /// Include patterns (gitignore-style), without leading '!'.
    pub walk_roots: GlobWalkRoot,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobSetError {
    #[error("failed to build globs")]
    BuildOverrides(#[source] ignore::Error),

    #[error("walk error at {0}")]
    Walk(PathBuf, #[source] ignore::Error),

    #[error(transparent)]
    WalkRoots(#[from] WalkRootsError),
}

impl GlobSet {
    /// Create a new [`GlobSet`] from a list of patterns. Leading '!' indicates exclusion.
    pub fn create<'t>(globs: impl IntoIterator<Item = &'t str>) -> GlobSet {
        GlobSet {
            walk_roots: GlobWalkRoot::build(globs).expect("should not fail"),
        }
    }

    /// Walks files matching all include/exclude patterns using a single parallel walker.
    /// Returns a flat Vec of results to keep lifetimes simple and predictable.
    pub fn collect_matching(&self, root_dir: &Path) -> Result<Vec<ignore::DirEntry>, GlobSetError> {
        if self.walk_roots.is_empty() {
            return Ok(vec![]);
        }

        let rebased = self.walk_roots.rebase(root_dir)?;
        walk::walk_globs(&rebased.root, &rebased.globs)
    }
}

#[cfg(test)]
mod tests {
    use super::GlobSet;
    use fs_err::{self as fs, File};
    use insta::assert_yaml_snapshot;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn relative_path(path: &Path, root: &Path) -> PathBuf {
        if let Ok(rel) = path.strip_prefix(root) {
            return rel.to_path_buf();
        }
        if let Some(parent) = root.parent() {
            if let Ok(rel) = path.strip_prefix(parent) {
                return std::path::Path::new("..").join(rel);
            }
        }
        path.to_path_buf()
    }

    fn sorted_paths(entries: Vec<ignore::DirEntry>, root: &std::path::Path) -> Vec<String> {
        let mut paths: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                relative_path(entry.path(), root)
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        paths.sort();
        paths
    }

    // Test out a normal non-reseated globbing approach
    #[test]
    fn collect_matching_inclusion_exclusion() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("include2.log")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();
        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["**/*.txt", "!exclude.txt"]);
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_yaml_snapshot!(paths, @r###"
        - include1.txt
        - subdir/include_subdir.txt
        "###);
    }

    // Check some general globbing support and make sure the correct things do not match
    #[test]
    fn collect_matching_relative_globs() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();
        let search_root = root_path.join("workspace");
        fs::create_dir(&search_root).unwrap();

        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/some_inner_source.cpp")).unwrap();
        File::create(root_path.join("subdir/dont-match.txt")).unwrap();
        File::create(search_root.join("match.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["../**/*.cpp", "*.txt"]);
        let entries = glob_set.collect_matching(&search_root).unwrap();

        let paths = sorted_paths(entries, &search_root);
        assert_yaml_snapshot!(paths, @r###"
        - "../subdir/some_inner_source.cpp"
        - match.txt
        "###);
    }

    // Check that single matching file glob works with rebasing
    #[test]
    fn collect_matching_file_glob() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml", "../*.cpp"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- pixi.toml");
    }

    // Check that global ignores !**/ patterns ignore everything even if the root has been
    // rebased to a parent folder, this is just a convenience assumed to be preferable
    // from a user standpoint
    #[test]
    fn check_global_ignore_ignores() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();
        File::create(root_path.join("foo.txt")).unwrap();
        // This would be picked up otherwise
        File::create(temp_dir.path().join("foo.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml", "!**/foo.txt"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- pixi.toml");
    }

    // Check that we can ignore a subset of file when using the rebasing
    // So we want to match all `.txt` and `*.toml` files except in the root location
    // where want to exclude `foo.txt`
    #[test]
    fn check_subset_ignore() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();
        // This should not be picked up
        File::create(root_path.join("foo.txt")).unwrap();
        // But because of the non-global ignore this should be
        File::create(temp_dir.path().join("foo.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["../*.{toml,txt}", "!foo.txt"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r###"
        - "../foo.txt"
        - pixi.toml
        "###);
    }

    /// Because we are using ignore which uses gitignore style parsing of globs we need to do some extra processing
    /// to make this more like unix globs in this case we check this explicitly here
    #[test]
    fn single_file_match() {
        let temp_dir = tempdir().unwrap();
        let workspace = temp_dir.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let subdir = workspace.join("subdir");
        fs::create_dir(&subdir).unwrap();

        File::create(subdir.join("pixi.toml")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml"]);
        let entries = glob_set.collect_matching(&workspace).unwrap();

        let paths = sorted_paths(entries, &workspace);
        assert_yaml_snapshot!(paths, @"[]");
    }
}
