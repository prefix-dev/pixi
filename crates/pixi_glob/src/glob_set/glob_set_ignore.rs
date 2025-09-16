use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::glob_set::walk;
use crate::glob_set::walk_roots::WalkRoots;

/// A glob set implemented using the `ignore` crate (globset + fast walker).
pub struct GlobSetIgnore {
    /// Include patterns (gitignore-style), without leading '!'.
    pub walk_roots: WalkRoots,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobSetIgnoreError {
    #[error("failed to build overrides")]
    BuildOverrides(#[source] ignore::Error),

    #[error("walk error at {0}")]
    Walk(PathBuf, #[source] ignore::Error),
}

impl GlobSetIgnore {
    /// Create a new `GlobSetIgnore` from a list of patterns. Leading '!' indicates exclusion.
    pub fn create<'t>(globs: impl IntoIterator<Item = &'t str>) -> GlobSetIgnore {
        GlobSetIgnore {
            walk_roots: WalkRoots::build(globs).expect("should not fail"),
        }
    }

    /// Walks files matching all include/exclude patterns using a single parallel walker.
    /// Returns a flat Vec of results to keep lifetimes simple and predictable.
    pub fn collect_matching(
        &self,
        root_dir: &Path,
    ) -> Result<Vec<ignore::DirEntry>, GlobSetIgnoreError> {
        if self.walk_roots.is_empty() {
            return Ok(vec![]);
        }

        let mut all_results = Vec::new();

        for walk_root in &self.walk_roots {
            let effective_walk_root = if walk_root.path().as_os_str().is_empty() {
                root_dir.to_path_buf()
            } else {
                root_dir.join(walk_root.path())
            };

            let globs: Vec<_> = walk_root.into_iter().collect();
            if globs.is_empty() {
                continue;
            }

            let mut results = walk::walk_globs(&effective_walk_root, &globs)?;
            all_results.append(&mut results);
        }

        Ok(all_results)
    }
}

#[cfg(test)]
mod tests {
    use super::GlobSetIgnore;
    use fs_err::{self as fs, File};
    use insta::assert_yaml_snapshot;
    use tempfile::tempdir;

    fn sorted_paths(entries: Vec<ignore::DirEntry>, root: &std::path::Path) -> Vec<String> {
        let mut paths: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn collect_matching_inclusion_exclusion() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("include2.log")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();
        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

        let glob_set = GlobSetIgnore::create(vec!["**/*.txt", "!exclude.txt"]);
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_yaml_snapshot!(paths, @r###"---
- include1.txt
- subdir/include_subdir.txt
"###);
    }

    #[test]
    fn collect_matching_relative_globs() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();
        let nested_root = root_path.join("workspace");
        fs::create_dir(&nested_root).unwrap();

        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/some_inner_source.cpp")).unwrap();

        let glob_set = GlobSetIgnore::create(vec!["../**/*.cpp"]);
        let entries = glob_set.collect_matching(&nested_root).unwrap();

        let paths = sorted_paths(entries, &nested_root);
        assert_yaml_snapshot!(paths, @r###"---
- "../subdir/some_inner_source.cpp"
"###);
    }

    #[test]
    fn collect_matching_file_glob() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("pixi.toml")).unwrap();

        let glob_set = GlobSetIgnore::create(vec!["pixi.toml"]);
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_yaml_snapshot!(paths, @r###"---
- pixi.toml
"###);
    }
}
